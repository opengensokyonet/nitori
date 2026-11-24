//! A call that puts requests for the connection in its box: taken from
//! the context after the poll, each with its place in the order.
use alloc::vec::Vec;

use frunk_core::{hlist, hlist_pat};

use super::{Q1, Q2, calls::Closing, cx, host, noop};
use crate::{
    call::{poll_call, start_calls, take_requests},
    lend::tests::fixtures::{
        Conn1, Conn2, LinkId, LinkPath, PathKey, same_on_both, tree_one_as_two, tree_two,
    },
};

#[test]
fn put_in_and_taken() {
    let mut root = tree_one_as_two();
    let mut cx1 = cx::<Q1>();
    let hlist_pat![_, _, _, closing, _, _] = start_calls::<Q1, LinkPath, _>(&mut 5);
    let mut closing = core::pin::pin!(closing);
    let polled = poll_call::<Q1, LinkPath, Closing, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(2)],
        &mut cx1,
        closing.as_mut(),
        &mut host(0),
        &mut noop(),
    );
    let one: (_, Vec<(u64, u32)>) = (
        polled.map(|p| p.is_ready()),
        take_requests::<Q1, Conn1, _>(&mut cx1)
            .into_iter()
            .map(|(n, r)| (n, r.request.0))
            .collect(),
    );
    assert!(take_requests::<Q1, Conn1, _>(&mut cx1).is_empty());

    let mut root = tree_two();
    let mut cx2 = cx::<Q2>();
    let hlist_pat![_, _, _, closing, _, _] = start_calls::<Q2, LinkPath, _>(&mut 5);
    let mut closing = core::pin::pin!(closing);
    let polled = poll_call::<Q2, LinkPath, Closing, _, _>(
        &mut root,
        &hlist![LinkId(2)],
        &mut cx2,
        closing.as_mut(),
        &mut host(0),
        &mut noop(),
    );
    let requests = take_requests::<Q2, Conn2, _>(&mut cx2);
    assert!(
        requests
            .iter()
            .all(|(_, r)| r.at == crate::structure::KeyAddress::here())
    );
    let two: (_, Vec<(u64, u32)>) = (
        polled.map(|p| p.is_ready()),
        requests
            .into_iter()
            .map(|(n, r)| (n, r.request.0))
            .collect(),
    );
    same_on_both(one, two, (Some(true), alloc::vec![(0, 2), (1, 102)]));
}
