//! The timer part among other parts: it has a number, so a component can
//! find another part's arguments, and the composition can have its own
//! list of contributions.
#![feature(impl_trait_in_assoc_type)]
mod common;

use std::{future::Future, pin::Pin, task::Context};

use nitori_driver::{
    capability::{Contribute, PartContribution, PiecesOf},
    component::{ArgsOf, Build, Component, FindArgs, Part, PartList, part_args},
    composition,
    driver::Driver,
    frunk_core::{HList, hlist, hlist::HNil},
    id::Numbered,
    need::{Pos, Ref, Requests, View},
    request::NodeRequest,
    structure::{KeyAddress, Share},
};

use common::*;

/// The part whose component reads its own arguments and another part's.
#[derive(Numbered)]
pub struct Base;
impl<L> Part<L> for Base {
    type Args = u64;
    type Shares = HList![Share<Leaf, HList![Reads]>];
}
/// The part whose arguments are read.
#[derive(Numbered)]
pub struct Other;
impl<L> Part<L> for Other {
    type Args = u32;
}
/// A leaf's component: what it was handed, and what it found.
#[derive(Numbered)]
pub struct Reads {
    own: u64,
    other: Option<u32>,
}
impl<L: PartList<L> + FindArgs<Other, L>> Build<Base, L, Leaf> for Reads {
    fn build<P: Pos>(_: &mut (), own: &u64, args: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
        Reads {
            own: *own,
            other: part_args::<Other, L>(args).copied(),
        }
    }
}

impl<L: PartList<L> + FindArgs<Other, L>> Component<Base, L, Leaf> for Reads {}

/// A family, and what the composition's list gives for `Other`.
#[derive(Numbered)]
pub struct Tally;
pub struct Mark;
pub struct OtherTally;
impl Contribute<Tally> for OtherTally {
    type Needs = HNil;
    type Piece<'t> = Mark;
    fn assemble<'t, P: Pos>(_: View<'t, Self::Needs, P>) -> Self::Piece<'t> {
        Mark
    }
}

pub struct Q;
composition!(
    impl for Q: Top,
    HList![Base, Other, nitori_driver_timer::Part<Q>],
    FakeClock,
    Level,
    HList![PartContribution<Other, Tally, OtherTally>],
);

/// The pieces of the family, in the order of the parts.
fn _lined_up<'t>(pieces: PiecesOf<'t, Q, Tally>) -> HList![(), Mark, ()] {
    pieces
}

#[test]
fn timer_part_with_part_args_and_listed_contributions() {
    let clock = FakeClock::default();
    let mut d = Driver::<Q>::new(clock, (), hlist![7, 9, ()], Level::Low);
    d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
        &KeyAddress::here(),
        |nitori_driver::frunk_core::hlist_pat![r]| r.push(NodeRequest::new(KeyAddress::here(), 1)),
    )
    .unwrap()
    .unwrap();
    let (_, w) = outer();
    let _ = Pin::new(&mut d).poll(&mut Context::from_waker(&w));
    let read = d
        .operate::<Leaf, HList![Ref<Reads>], _, _, _>(
            &KeyAddress::<Top, Top>::child::<Leaf, _>(1),
            |nitori_driver::frunk_core::hlist_pat![r]| (r.own, r.other),
        )
        .unwrap()
        .unwrap();
    assert_eq!(read, (7, Some(9)));
}
