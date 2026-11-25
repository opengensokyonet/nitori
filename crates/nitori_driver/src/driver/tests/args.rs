//! What `build` is handed as `own`, through the driver: the arguments of
//! the component's own part, at the root and at a node added below it.
use frunk_core::{HList, hlist, hlist_pat};

use crate::{
    component::{ArgsOf, Component, Part, PartList},
    composition,
    driver::{Driver, Edit, HandlerFor},
    id::Numbered,
    lend::tests::fixtures::TestClock,
    need::{Pos, Ref, Requests, View},
    request::{HandleRequest, NodeRequest},
    structure::{Child, KeyAddress, NodeKind, Share, SlotVec},
};

/// The root: leaves by number; it is asked to add one.
#[derive(Numbered)]
struct Top;
impl NodeKind for Top {
    type Children = HList![Child<Leaf, SlotVec<u32>>];
    type Request = u32;
    type Handler = Adding;
}
#[derive(Numbered)]
struct Leaf;
impl NodeKind for Leaf {
    type Children = HList![];
}
/// Adds the leaf of the number it is asked.
struct Adding;
impl HandleRequest<Top, u8> for Adding {
    type Needs = HList![];
    fn handle<P: HandlerFor<Top, Self, u8>>(mut edit: Edit<'_, Top, P>, key: u32) {
        let _ = edit.add::<Leaf, _>(key, (), 0);
    }
}

/// A component that keeps what it was handed as `own`, for every kind.
macro_rules! keeps {
    ($($name:ident: $part:ty, $args:ty;)*) => {$(
        #[derive(Numbered)]
        struct $name($args);
        impl<L: PartList<L>, K: NodeKind> Component<$part, L, K> for $name {
            fn build<P: Pos>(_: &mut K::Start, own: &$args, _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self {
                $name(*own)
            }
        }
    )*};
}
keeps! {
    TopFirst: First, u32;
    TopSecond: Second, &'static str;
    LeafFirst: First, u32;
    LeafSecond: Second, &'static str;
}

/// Two parts, with arguments of two types.
#[derive(Numbered)]
struct First;
impl<L> Part<L> for First {
    type Args = u32;
    type Shares = HList![Share<Top, HList![TopFirst]>, Share<Leaf, HList![LeafFirst]>];
}
#[derive(Numbered)]
struct Second;
impl<L> Part<L> for Second {
    type Args = &'static str;
    type Shares = HList![Share<Top, HList![TopSecond]>, Share<Leaf, HList![LeafSecond]>];
}

struct QA;
composition!(impl for QA: Top, HList![First, Second], TestClock, u8);

#[test]
fn build_is_handed_its_own_parts_arguments() {
    let clock = TestClock {
        last: core::time::Duration::from_secs(10),
    };
    let mut d = Driver::<QA>::new(clock, (), hlist![5, "second"], 0);
    let at_top = d
        .operate::<Top, HList![Ref<TopFirst>, Ref<TopSecond>], _, _, _>(
            &KeyAddress::here(),
            |hlist_pat![a, b]| (a.0, b.0),
        )
        .unwrap()
        .unwrap();
    assert_eq!(at_top, (5, "second"));

    d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![r]| r.push(NodeRequest::new(KeyAddress::here(), 1)),
    )
    .unwrap()
    .unwrap();
    let (_, w) = super::outer();
    let _ = super::poll(&mut d, &w);
    let at_leaf = d
        .operate::<Leaf, HList![Ref<LeafFirst>, Ref<LeafSecond>], _, _, _>(
            &KeyAddress::<Top, Top>::child::<Leaf, _>(1),
            |hlist_pat![a, b]| (a.0, b.0),
        )
        .unwrap()
        .unwrap();
    assert_eq!(at_leaf, (5, "second"));
}
