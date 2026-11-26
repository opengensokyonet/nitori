//! A node moved with nodes below it: their keys in the driver's table
//! move with it, so they are found at the new address and end for it.
//!
//! The structure: `Root { [Mid by u32 { [Arm by u32 { [Bud by u32] }] }] }`;
//! the root is asked to move an arm from a mid to another; a bud keeps a
//! component that notes its end.
extern crate std;

use alloc::format;

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    core,
    fixture::{clock, note, notes},
};
use crate::{
    component::{ArgsOf, Build, Component, ComponentView, EndCause, Part, PartList},
    composition,
    driver::{Driver, Edit, HandlerFor},
    id::Numbered,
    need::{OwnKey, Pos, Ref, Requests, View},
    request::{HandleRequest, NodeRequest},
    structure::{Child, KeyAddress, NodeKind, Share, SlotVec},
};

#[derive(Numbered)]
pub struct Root;
#[derive(Numbered)]
pub struct Mid;
#[derive(Numbered)]
pub struct Arm;
#[derive(Numbered)]
pub struct Bud;
impl NodeKind for Root {
    type Children = HList![Child<Mid, SlotVec<u32>>];
    type Request = Shift;
    type Handler = Shifter;
}
impl NodeKind for Mid {
    type Children = HList![Child<Arm, SlotVec<u32>>];
}
impl NodeKind for Arm {
    type Children = HList![Child<Bud, SlotVec<u32>>];
}
impl NodeKind for Bud {
    type Children = HList![];
}

/// What the root is asked: to move the arm from a mid to another.
pub struct Shift {
    arm: u32,
    from: u32,
    to: u32,
}
/// The root's handler: it moves the arm and notes how it went.
pub struct Shifter;
impl<Pr> HandleRequest<Root, Pr> for Shifter {
    type Needs = HList![];
    fn handle<P: HandlerFor<Root, Self, Pr>>(mut edit: Edit<'_, Root, P>, r: Shift) {
        let out = edit.move_grandchild::<Mid, _, Arm, _>(&r.from, &r.arm, &r.to);
        note(format!("move: {out:?}"));
    }
}

/// A bud's component: its key.
#[derive(Numbered)]
pub struct Mark {
    key: u32,
}
impl<L: PartList<L>> Build<Deep, L, Bud> for Mark {
    type BuildNeeds = HList![OwnKey<u32>];
    fn build<P: Pos>(
        _: &mut (),
        _: &(),
        _: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self {
        let hlist_pat![key] = view;
        Mark { key }
    }
}

impl<L: PartList<L>> Component<Deep, L, Bud> for Mark {
    fn end<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>, cause: EndCause<'_, ()>) {
        let hlist_pat![me] = view;
        note(format!("end bud {} {cause:?}", me.key));
    }
}

/// The one part.
#[derive(Numbered)]
pub struct Deep;
impl<L> Part<L> for Deep {
    type Args = ();
    type Shares = HList![Share<Bud, HList![Mark]>];
}

pub struct QD;
composition!(impl for QD: Root, HList![Deep], crate::lend::tests::fixtures::TestClock, u8);

fn bud_at(mid: u32, arm: u32, bud: u32) -> KeyAddress<Root, Bud> {
    KeyAddress::<Root, Root>::below::<Mid, _, Bud>(
        mid,
        KeyAddress::<Mid, Mid>::below::<Arm, _, Bud>(
            arm,
            KeyAddress::<Arm, Arm>::child::<Bud, _>(bud),
        ),
    )
}
fn mark_at(
    d: &mut Driver<QD>,
    at: &KeyAddress<Root, Bud>,
) -> Result<Option<u32>, crate::driver::OperateError> {
    d.operate::<Bud, HList![Ref<Mark>], _, _, _>(at, |hlist_pat![m]| m.key)
}

/// b: a bud below the arm moved is borrowed at the new address and not at
/// the old one, and ends, when its new mid ends, as a node of it.
#[test]
fn nodes_below_the_one_moved_move_their_keys() {
    notes();
    let mut d = Driver::<QD>::new(clock(), (), hlist![()], 0);
    let c = core(&mut d);
    c.add::<Mid, _>(hlist![1], (), 0).unwrap();
    let to = c.add::<Mid, _>(hlist![2], (), 0).unwrap();
    c.add::<Arm, _>(hlist![1, 5], (), 0).unwrap();
    c.add::<Bud, _>(hlist![1, 5, 7], (), 0).unwrap();
    assert_eq!(mark_at(&mut d, &bud_at(1, 5, 7)), Ok(Some(7)));
    let out = d.operate::<Root, HList![Requests<NodeRequest<Root, Root>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![roots]| {
            roots.push(NodeRequest::new(
                KeyAddress::here(),
                Shift {
                    arm: 5,
                    from: 1,
                    to: 2,
                },
            ));
        },
    );
    assert_eq!(out, Ok(Some(())));
    assert_eq!(notes(), ["move: Ok(())"]);
    assert_eq!(mark_at(&mut d, &bud_at(1, 5, 7)), Ok(None));
    assert_eq!(mark_at(&mut d, &bud_at(2, 5, 7)), Ok(Some(7)));
    core(&mut d).end_for::<Mid, _>(to, &());
    assert_eq!(notes(), ["end bud 7 Above"]);
    drop(d);
    assert!(notes().is_empty());
}
