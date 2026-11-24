//! The external operation: a lending at a node by its key address, and
//! what it is refused.
extern crate std;

use alloc::vec;
use core::sync::atomic::Ordering;

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    core,
    fixture::{Leaf, Level, QT, Step, Top, TopAsk, clock, notes},
    outer, poll,
};
use crate::{
    call::tests::{Q1, Q2},
    driver::{Driver, OperateError},
    lend::tests::fixtures::{CloseConn, Conn1, Conn2, LinkCore, LinkId, LinkPath, TestClock},
    need::{Mut, Requests},
    request::NodeRequest,
    structure::KeyAddress,
};

#[test]
fn it_lends_at_the_node_or_finds_none() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(hlist![1], vec![], Level::Low)
        .unwrap();
    let at = |k| KeyAddress::<Top, Top>::child::<Leaf, _>(k);
    let key =
        d.operate::<Leaf, HList![Mut<super::fixture::Ctl>], _, _, _>(&at(1), |hlist_pat![c]| c.key);
    assert_eq!(key, Ok(Some(1)));
    let none =
        d.operate::<Leaf, HList![Mut<super::fixture::Ctl>], _, _, _>(&at(9), |hlist_pat![c]| c.key);
    assert_eq!(none, Ok(None));
}

#[test]
fn it_wakes_the_driver_when_something_is_ready() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    let (count, w) = outer();
    let _ = poll(&mut d, &w);
    let before = count.0.load(Ordering::SeqCst);
    let out = d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![tops]| {
            tops.push(NodeRequest::new(
                KeyAddress::here(),
                TopAsk::Add(3, vec![], Level::Low),
            ))
        },
    );
    assert_eq!(out, Ok(Some(())));
    assert!(count.0.load(Ordering::SeqCst) > before);
    notes();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 3 #1"]);
}

#[test]
fn after_a_panic_it_is_refused() {
    notes();
    let mut d = Driver::<QT>::new(clock(), (), hlist![()], Level::Low);
    core(&mut d)
        .add::<Leaf, _>(hlist![1], vec![Step::Panic], Level::Low)
        .unwrap();
    let (_, w) = outer();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| poll(&mut d, &w)));
    assert!(unwound.is_err());
    let out = d.operate::<Top, HList![], _, _, _>(&KeyAddress::here(), |_| ());
    assert_eq!(out, Err(OperateError::Panicked));
    notes();
    drop(d);
    assert!(notes().is_empty());
}

#[test]
fn asked_to_close_either_connection_ends_the_driver() {
    let args = || {
        hlist![
            crate::lend::tests::parts::CoreArgs,
            crate::lend::tests::parts::LossArgs { window: 4 },
            crate::lend::tests::parts::WatchArgs
        ]
    };
    let clock = || TestClock {
        last: core::time::Duration::from_secs(10),
    };
    let (_, w) = outer();
    let mut d = Driver::<Q1>::new(clock(), (), args(), 0);
    let out = d.operate::<Conn1, HList![Requests<NodeRequest<Conn1, Conn1>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![r]| r.push(NodeRequest::new(KeyAddress::here(), CloseConn(1))),
    );
    assert_eq!(out, Ok(Some(())));
    assert!(poll(&mut d, &w).is_ready());
    let mut d = Driver::<Q2>::new(clock(), (), args(), 0);
    core(&mut d)
        .add::<LinkPath, _>(hlist![LinkId(1)], 5, 0)
        .unwrap();
    // a link path's core, lent from outside, then the connection closed
    let id = d.operate::<LinkPath, HList![Mut<LinkCore>], _, _, _>(
        &KeyAddress::<Conn2, Conn2>::child::<LinkPath, _>(LinkId(1)),
        |hlist_pat![c]| c.rx,
    );
    assert!(matches!(id, Ok(Some(_))));
    let out = d.operate::<Conn2, HList![Requests<NodeRequest<Conn2, Conn2>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![r]| r.push(NodeRequest::new(KeyAddress::here(), CloseConn(1))),
    );
    assert_eq!(out, Ok(Some(())));
    assert_eq!(
        d.operate::<Conn2, HList![], _, _, _>(&KeyAddress::here(), |_| ()),
        Err(OperateError::Ended)
    );
    assert!(poll(&mut d, &w).is_ready());
}
