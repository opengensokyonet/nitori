//! The operations of components: building nodes from the nodes above
//! them, waking a component and ending it, on both structures.
use alloc::{string::String, vec::Vec};

use frunk_core::{HList, hlist, hlist::HNil, hlist_pat};

use super::{fixtures::*, parts::*};
use crate::{
    component::{Build, EndCause},
    lend::entry::{build_node, build_root, end, wake},
    need::{EmptyPos, Ref},
    structure::{End, Node, insert},
};

fn args() -> HList![CoreArgs, LossArgs, WatchArgs] {
    hlist![CoreArgs, LossArgs { window: 4 }, WatchArgs]
}

/// The link core reads the loss part's window, which a list without the
/// loss part does not have.
#[test]
fn generic_over_the_parts() {
    let with = <LinkCore as Build<Core, Parts, LinkPath>>::build::<EmptyPos>(
        &mut 0,
        &CoreArgs,
        &args(),
        HNil,
    );
    let without = <LinkCore as Build<Core, HList![Core, Watch], LinkPath>>::build::<EmptyPos>(
        &mut 0,
        &CoreArgs,
        &hlist![CoreArgs, WatchArgs],
        HNil,
    );
    assert_eq!((with.rx, without.rx), (4, 0));
}

/// What building a link path and a recovery below it gave: the link's
/// core, planner and estimator, the start after it, the congestion, the
/// factory's count, and a recovery below a link path that is not there.
type Built = ((u32, u64, u32), u64, u32, u32, bool);

#[test]
fn build_from_the_nodes_above() {
    let mut start = 5;
    let mut root = Node::<Conn1, Parts>::new(build_root::<Conn1, Parts>(&mut (), &args()));
    let data = build_node::<_, _, LogicalPath, _, _, _>(
        &mut root,
        &hlist![PathKey(10)],
        &mut (),
        &args(),
        &mut (),
    )
    .expect("the root is there");
    let path = insert(
        &mut root,
        End,
        PathKey(10),
        Node::<LogicalPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    let data = build_node::<_, _, LinkPath, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(1)],
        &mut start,
        &args(),
        &mut (),
    )
    .expect("the logical path is there");
    let hlist_pat![hlist_pat![core, planner], hlist_pat![], hlist_pat![rtt]] = &data;
    let link = (core.rx, planner.next_pn, rtt.0.samples);
    let at = insert(
        &mut root,
        path,
        LinkId(1),
        Node::<LinkPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    let data = build_node::<_, _, Recovery, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(1)],
        &mut (),
        &args(),
        &mut (),
    )
    .expect("the link path is there");
    let congestion = data.tail.head.head.in_flight;
    insert(&mut root, at, (), Node::<Recovery, Parts>::new(data))
        .ok()
        .expect("the key is free");
    let missing = build_node::<_, _, Recovery, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(9)],
        &mut (),
        &args(),
        &mut (),
    )
    .is_none();
    let made = lend!(
        &mut root,
        Conn1,
        HList![Ref<RecoveryFactory>],
        hlist![],
        |hlist_pat![f]| f.made
    )
    .expect("the root is there");
    let one: Built = (link, start, congestion, made, missing);

    let mut start = 5;
    let mut root = Node::<Conn2, Parts>::new(build_root::<Conn2, Parts>(&mut (), &args()));
    let data = build_node::<_, _, LinkPath, _, _, _>(
        &mut root,
        &hlist![LinkId(1)],
        &mut start,
        &args(),
        &mut (),
    )
    .expect("the root is there");
    let hlist_pat![hlist_pat![core, planner], hlist_pat![], hlist_pat![rtt]] = &data;
    let link = (core.rx, planner.next_pn, rtt.0.samples);
    let at = insert(
        &mut root,
        End,
        LinkId(1),
        Node::<LinkPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    let data = build_node::<_, _, Recovery, _, _, _>(
        &mut root,
        &hlist![LinkId(1)],
        &mut (),
        &args(),
        &mut (),
    )
    .expect("the link path is there");
    let congestion = data.tail.head.head.in_flight;
    insert(&mut root, at, (), Node::<Recovery, Parts>::new(data))
        .ok()
        .expect("the key is free");
    let missing = build_node::<_, _, Recovery, _, _, _>(
        &mut root,
        &hlist![LinkId(9)],
        &mut (),
        &args(),
        &mut (),
    )
    .is_none();
    let made = lend!(
        &mut root,
        Conn2,
        HList![Ref<RecoveryFactory>],
        hlist![],
        |hlist_pat![f]| f.made
    )
    .expect("the root is there");
    let two: Built = (link, start, congestion, made, missing);

    // The planner counts from the start, its key and the epoch (0), and
    // takes one of the start; the congestion starts at the planner's.
    same_on_both(one, two, ((4, 105, 1), 6, 105, 1, true));
}

/// What waking link 1's core did: its count and the factory's.
type Woken = (Option<()>, Option<(u32, u32)>, Option<()>);

#[test]
fn wake_the_component_with_its_needs() {
    let mut root = tree_one_as_two();
    let woke = wake::<_, _, LinkPath, _, Core, LinkCore, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(1)],
        &mut (),
    );
    let after = lend!(
        &mut root,
        LinkPath,
        HList![Ref<LinkCore>, Ref<RecoveryFactory>],
        hlist![PathKey(10), LinkId(1)],
        |hlist_pat![l, f]| (l.rx, f.made)
    );
    let absent = wake::<_, _, LinkPath, _, Core, LinkCore, _, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(9)],
        &mut (),
    );
    let one: Woken = (woke, after, absent);
    let mut root = tree_two();
    let woke =
        wake::<_, _, LinkPath, _, Core, LinkCore, _, _, _>(&mut root, &hlist![LinkId(1)], &mut ());
    let after = lend!(
        &mut root,
        LinkPath,
        HList![Ref<LinkCore>, Ref<RecoveryFactory>],
        hlist![LinkId(1)],
        |hlist_pat![l, f]| (l.rx, f.made)
    );
    let absent =
        wake::<_, _, LinkPath, _, Core, LinkCore, _, _, _>(&mut root, &hlist![LinkId(9)], &mut ());
    let two: Woken = (woke, after, absent);
    same_on_both(one, two, (Some(()), Some((3, 8)), None));
}

#[test]
fn end_with_each_cause() {
    let causes = [EndCause::Reason(&7), EndCause::Above, EndCause::Dropped];
    let mut root = tree_one_as_two();
    for cause in causes {
        end::<_, _, LinkPath, _, Core, Planner, _, _, _>(
            &mut root,
            &hlist![PathKey(10), LinkId(2)],
            &mut (),
            cause,
        )
        .expect("link 2 is there");
    }
    let one = take_log();
    let mut root = tree_two();
    for cause in causes {
        end::<_, _, LinkPath, _, Core, Planner, _, _, _>(
            &mut root,
            &hlist![LinkId(2)],
            &mut (),
            cause,
        )
        .expect("link 2 is there");
    }
    let two = take_log();
    let want: Vec<String> = [
        "end Reason(7) pn=200 rx=0",
        "end Above pn=200 rx=0",
        "end Dropped pn=200 rx=0",
    ]
    .map(String::from)
    .into();
    same_on_both(one, two, want);
}
