//! A call that waits for something outside the tree: pending while it
//! waits, ready once it returned; and a call polled at a node that is not
//! there.
use frunk_core::{HList, hlist, hlist_pat};

use super::{
    Q1, Q2,
    calls::{Waiting, set_gate},
    cx, host, noop,
};
use crate::{
    call::{poll_call, start_calls},
    lend::tests::fixtures::{
        LinkCore, LinkId, LinkPath, PathKey, lend, same_on_both, tree_one_as_two, tree_two,
    },
    need::Ref,
};

/// What the waiting call did: the first poll, the count after it, the
/// second poll after the gate opened and the count after it.
type Waited = (Option<bool>, Option<u32>, Option<bool>, Option<u32>);

#[test]
fn pending_then_ready() {
    set_gate(false);
    let mut root = tree_one_as_two();
    let mut cx1 = cx::<Q1>();
    let at = hlist![PathKey(10), LinkId(2)];
    let hlist_pat![_, _, waiting, _, _, _] = start_calls::<Q1, LinkPath, _>(&mut 5);
    let mut waiting = core::pin::pin!(waiting);
    let first = poll_call::<Q1, LinkPath, Waiting, _, _>(
        &mut root,
        &at,
        &mut cx1,
        waiting.as_mut(),
        &mut host(0),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let before = lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
        c
    ]| c.rx);
    set_gate(true);
    let second = poll_call::<Q1, LinkPath, Waiting, _, _>(
        &mut root,
        &at,
        &mut cx1,
        waiting.as_mut(),
        &mut host(0),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let after = lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
        c
    ]| c.rx);
    let one: Waited = (first, before, second, after);

    set_gate(false);
    let mut root = tree_two();
    let mut cx2 = cx::<Q2>();
    let at = hlist![LinkId(2)];
    let hlist_pat![_, _, waiting, _, _, _] = start_calls::<Q2, LinkPath, _>(&mut 5);
    let mut waiting = core::pin::pin!(waiting);
    let first = poll_call::<Q2, LinkPath, Waiting, _, _>(
        &mut root,
        &at,
        &mut cx2,
        waiting.as_mut(),
        &mut host(0),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let before = lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
        c
    ]| c.rx);
    set_gate(true);
    let second = poll_call::<Q2, LinkPath, Waiting, _, _>(
        &mut root,
        &at,
        &mut cx2,
        waiting.as_mut(),
        &mut host(0),
        &mut noop(),
    )
    .map(|p| p.is_ready());
    let after = lend!(&mut root, LinkPath, HList![Ref<LinkCore>], at, |hlist_pat![
        c
    ]| c.rx);
    let two: Waited = (first, before, second, after);

    same_on_both(one, two, (Some(false), Some(0), Some(true), Some(100)));
}

#[test]
fn not_there() {
    let mut root = tree_two();
    let mut cx2 = cx::<Q2>();
    let hlist_pat![_, _, waiting, _, _, _] = start_calls::<Q2, LinkPath, _>(&mut 5);
    let mut waiting = core::pin::pin!(waiting);
    let polled = poll_call::<Q2, LinkPath, Waiting, _, _>(
        &mut root,
        &hlist![LinkId(9)],
        &mut cx2,
        waiting.as_mut(),
        &mut host(0),
        &mut noop(),
    );
    assert!(polled.is_none());
}
