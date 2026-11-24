//! A new node's calls start after its components are built, from what
//! the components left of its start, in the order of the parts and then
//! of each part's calls: the core's budget, then its waiting call, then
//! the loss part's budget.
use alloc::{string::String, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::{Q1, Q2, calls::BudgetA, cx, noop};
use crate::{
    call::{Composition, build, poll_call, start_calls},
    component::ArgsOf,
    lend::tests::{
        fixtures::{LinkId, LinkPath, LogicalPath, Parts, PathKey, Planner, lend, same_on_both},
        parts::{CoreArgs, LossArgs, WatchArgs, take_log},
    },
    need::Ref,
    structure::{End, Node, insert},
};

pub fn args() -> ArgsOf<Parts> {
    hlist![CoreArgs, LossArgs { window: 4 }, WatchArgs]
}

/// What starting link 7's calls did: what they noted of the start, the
/// start after them, and the planner after the first budget's poll.
type Started = (Vec<String>, u64, Option<u64>);

#[test]
fn components_first_then_calls() {
    let mut cx1 = cx::<Q1>();
    let mut root = Q1::root(&mut (), &args());
    let data =
        build::<Q1, LogicalPath, _>(&mut root, &hlist![PathKey(10)], &mut (), &args(), &mut cx1)
            .expect("the root is there");
    let path = insert(
        &mut root,
        End,
        PathKey(10),
        Node::<LogicalPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    let at = hlist![PathKey(10), LinkId(7)];
    let mut start = 5;
    let data = build::<Q1, LinkPath, _>(&mut root, &at, &mut start, &args(), &mut cx1)
        .expect("the logical path is there");
    insert(
        &mut root,
        path,
        LinkId(7),
        Node::<LinkPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    let hlist_pat![_, budget_a, _, _, _, _] = start_calls::<Q1, LinkPath, _>(&mut start);
    let log = take_log();
    let mut budget_a = core::pin::pin!(budget_a);
    poll_call::<Q1, LinkPath, BudgetA, _, _>(
        &mut root,
        &at,
        &mut cx1,
        budget_a.as_mut(),
        &mut noop(),
    );
    let pn = lend!(&mut root, LinkPath, HList![Ref<Planner>], at, |hlist_pat![
        p
    ]| p
        .next_pn);
    let one: Started = (log, start, pn);

    let mut cx2 = cx::<Q2>();
    let mut root = Q2::root(&mut (), &args());
    let at = hlist![LinkId(7)];
    let mut start = 5;
    let data = build::<Q2, LinkPath, _>(&mut root, &at, &mut start, &args(), &mut cx2)
        .expect("the root is there");
    insert(
        &mut root,
        End,
        LinkId(7),
        Node::<LinkPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    let hlist_pat![_, budget_a, _, _, _, _] = start_calls::<Q2, LinkPath, _>(&mut start);
    let log = take_log();
    let mut budget_a = core::pin::pin!(budget_a);
    poll_call::<Q2, LinkPath, BudgetA, _, _>(
        &mut root,
        &at,
        &mut cx2,
        budget_a.as_mut(),
        &mut noop(),
    );
    let pn = lend!(&mut root, LinkPath, HList![Ref<Planner>], at, |hlist_pat![
        p
    ]| p
        .next_pn);
    let two: Started = (log, start, pn);

    // The planner counts from the start and takes one of it; the core's
    // budget gets what is left and takes ten, the core's waiting call
    // doubles what is left after it, and the loss part's budget comes last.
    same_on_both(
        one,
        two,
        (
            ["start a 6", "start waiting 16", "start b 32"]
                .map(String::from)
                .to_vec(),
            32,
            Some(711),
        ),
    );
}
