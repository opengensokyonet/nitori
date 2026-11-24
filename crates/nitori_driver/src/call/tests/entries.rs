//! The other entries of the pack: the driver's reaction points at a node
//! just added and about to end, and a component woken and ended, on both
//! structures.
use alloc::{string::String, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::{Q1, Q2, cx, start::args};
use crate::{
    call::{Composition, build, end, node_ending, node_started, wake},
    component::EndCause,
    lend::tests::{
        fixtures::{
            Core, LinkCore, LinkId, LinkPath, LogicalPath, Parts, PathKey, Planner,
            RecoveryFactory, lend, same_on_both,
        },
        parts::take_log,
    },
    need::{At, Ref},
    structure::{End, Node, insert},
};

/// What link 7 went through: the log, the link's count and the factory's,
/// and a wake at a link that is not there.
type Lived = (Vec<String>, Option<(u32, u32)>, bool);

#[test]
fn started_woken_ended() {
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
    let data = build::<Q1, LinkPath, _>(&mut root, &at, &mut 5, &args(), &mut cx1)
        .expect("the logical path is there");
    insert(
        &mut root,
        path,
        LinkId(7),
        Node::<LinkPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    take_log();
    node_started::<Q1, LinkPath, _>(&mut root, &at, &mut cx1).expect("the link is there");
    wake::<Q1, LinkPath, Core, LinkCore, _, _>(&mut root, &at, &mut cx1)
        .expect("the link is there");
    end::<Q1, LinkPath, Core, Planner, _, _>(&mut root, &at, &mut cx1, EndCause::Reason(&3))
        .expect("the link is there");
    node_ending::<Q1, LinkPath, _>(&mut root, &at, &mut cx1).expect("the link is there");
    let missing = wake::<Q1, LinkPath, Core, LinkCore, _, _>(
        &mut root,
        &hlist![PathKey(10), LinkId(9)],
        &mut cx1,
    )
    .is_none();
    let counts = lend!(
        &mut root,
        LinkPath,
        HList![Ref<LinkCore>, At<RecoveryFactory, Ref<RecoveryFactory>>],
        at,
        |hlist_pat![l, f]| (l.rx, f.made)
    );
    let one: Lived = (take_log(), counts, missing);

    let mut cx2 = cx::<Q2>();
    let mut root = Q2::root(&mut (), &args());
    let at = hlist![LinkId(7)];
    let data = build::<Q2, LinkPath, _>(&mut root, &at, &mut 5, &args(), &mut cx2)
        .expect("the root is there");
    insert(
        &mut root,
        End,
        LinkId(7),
        Node::<LinkPath, Parts>::new(data),
    )
    .ok()
    .expect("the key is free");
    take_log();
    node_started::<Q2, LinkPath, _>(&mut root, &at, &mut cx2).expect("the link is there");
    wake::<Q2, LinkPath, Core, LinkCore, _, _>(&mut root, &at, &mut cx2)
        .expect("the link is there");
    end::<Q2, LinkPath, Core, Planner, _, _>(&mut root, &at, &mut cx2, EndCause::Reason(&3))
        .expect("the link is there");
    node_ending::<Q2, LinkPath, _>(&mut root, &at, &mut cx2).expect("the link is there");
    let missing =
        wake::<Q2, LinkPath, Core, LinkCore, _, _>(&mut root, &hlist![LinkId(9)], &mut cx2)
            .is_none();
    let counts = lend!(
        &mut root,
        LinkPath,
        HList![Ref<LinkCore>, At<RecoveryFactory, Ref<RecoveryFactory>>],
        at,
        |hlist_pat![l, f]| (l.rx, f.made)
    );
    let two: Lived = (take_log(), counts, missing);

    // The link core starts at the loss part's window and adds the key
    // epoch (0) when woken; the factory counts the planner's build and the
    // wake.
    same_on_both(
        one,
        two,
        (
            [
                "started@conn 7",
                "started@link 705",
                "end Reason(3) pn=705 rx=4",
                "ending@conn 4",
            ]
            .map(String::from)
            .to_vec(),
            Some((4, 2)),
            true,
        ),
    );
}
