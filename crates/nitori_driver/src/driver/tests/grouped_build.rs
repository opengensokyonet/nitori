//! Grouped construction and the ordinary operations of its members.
use alloc::vec::Vec;
use core::task::Waker;

use frunk_core::{HList, hlist, hlist_pat};

use super::fixture::clock;
use crate::{
    component::{
        ArgsOf, Build, Component, ComponentView, Constructors, EndCause, Group, One, Part, PartList,
    },
    composition,
    driver::{Driver, Edit, HandlerFor},
    id::Numbered,
    lend::tests::fixtures::TestClock,
    need::{Mut, OwnKey, Pos, Ref, Requests, View, WakerOf, Wakes},
    request::{HandleRequest, NodeRequest},
    structure::{Child, KeyAddress, NodeKind, Share, SlotVec},
};

#[derive(Numbered)]
struct Root;
impl NodeKind for Root {
    type Children = HList![Child<Leaf, SlotVec<u32>>];
    type Request = Change;
    type Handler = Changing;
}

#[derive(Numbered)]
struct Leaf;
impl NodeKind for Leaf {
    type Children = HList![];
    type Start = Start;
    type EndReason = u8;
}

enum Change {
    Add(u32, Start),
    End(u32, u8),
}

struct Changing;
impl HandleRequest<Root, u8> for Changing {
    type Needs = HList![];

    fn handle<P: HandlerFor<Root, Self, u8>>(mut edit: Edit<'_, Root, P>, change: Change) {
        match change {
            Change::Add(key, start) => {
                let _ = edit.add::<Leaf, _>(key, start, 0);
            }
            Change::End(key, reason) => {
                let _ = edit.end::<Leaf, _>(&key, reason);
            }
        }
    }
}

// The two tokens can only leave Start by value.
struct Token {
    id: u32,
}

struct Tokens {
    primary: Token,
    secondary: Token,
}

struct Start {
    tokens: Option<Tokens>,
    prepared: bool,
}
impl Start {
    fn new(primary: u32, secondary: u32) -> Self {
        Self {
            tokens: Some(Tokens {
                primary: Token { id: primary },
                secondary: Token { id: secondary },
            }),
            prepared: false,
        }
    }
}

#[derive(Default, Numbered)]
struct Factory {
    built: Vec<u32>,
    ended: Vec<(u32, u8)>,
}
impl<O: Part<L, Args = ()>, L: PartList<L>> Component<O, L, Root> for Factory {}
impl<O: Part<L, Args = ()>, L: PartList<L>> Build<O, L, Root> for Factory {
    fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self {
        Self::default()
    }
}

#[derive(Numbered)]
struct Ready;
impl<L: PartList<L>> Component<Mixed, L, Root> for Ready {}

struct MakeRoot;
impl<L: PartList<L>> Build<Mixed, L, Root> for MakeRoot {
    type Output = HList![Factory, Ready];

    fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self::Output {
        hlist![Factory::default(), Ready]
    }
}

#[derive(Numbered)]
struct Primary {
    token: Token,
    wakes: u32,
}
impl<O: Part<L>, L: PartList<L>> Component<O, L, Leaf> for Primary {
    type Needs = HList![Mut<Factory>];

    fn wake<P: Pos>(hlist_pat![primary, _]: ComponentView<'_, Self, Self::Needs, P>) {
        primary.wakes += 1;
    }

    fn end<P: Pos>(
        hlist_pat![primary, factory]: ComponentView<'_, Self, Self::Needs, P>,
        cause: EndCause<'_, u8>,
    ) {
        if let EndCause::Reason(reason) = cause {
            factory.ended.push((primary.token.id, *reason));
        }
    }
}
impl<L: PartList<L>> Build<Single, L, Leaf> for Primary {
    fn build<P: Pos>(start: &mut Start, _: &(), _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self {
        Self {
            token: start.tokens.take().unwrap().primary,
            wakes: 0,
        }
    }
}

#[derive(Numbered)]
struct Secondary(Token);
impl<L: PartList<L>> Component<Mixed, L, Leaf> for Secondary {}

#[derive(Numbered)]
struct Before;
impl<L: PartList<L>> Component<Mixed, L, Leaf> for Before {}
impl<L: PartList<L>> Build<Mixed, L, Leaf> for Before {
    fn build<P: Pos>(start: &mut Start, _: &(), _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self {
        assert!(start.tokens.is_some());
        start.prepared = true;
        Self
    }
}

#[derive(Numbered)]
struct After;
impl<L: PartList<L>> Component<Mixed, L, Leaf> for After {}
impl<L: PartList<L>> Build<Mixed, L, Leaf> for After {
    fn build<P: Pos>(start: &mut Start, _: &(), _: &ArgsOf<L>, _: View<'_, HList![], P>) -> Self {
        assert!(start.tokens.is_none());
        Self
    }
}

struct Both;
impl<L: PartList<L>> Build<Mixed, L, Leaf> for Both {
    type BuildNeeds = HList![Mut<Factory>, OwnKey<u32>];
    type Output = HList![Primary, Secondary];

    fn build<P: Pos>(
        start: &mut Start,
        _: &(),
        _: &ArgsOf<L>,
        hlist_pat![factory, key]: View<'_, Self::BuildNeeds, P>,
    ) -> Self::Output {
        assert!(start.prepared);
        factory.built.push(key);
        let Tokens { primary, secondary } = start.tokens.take().unwrap();
        hlist![
            Primary {
                token: primary,
                wakes: 0,
            },
            Secondary(secondary)
        ]
    }
}

#[derive(Numbered)]
struct Mixed;
impl<L> Part<L> for Mixed {
    type Args = ();
    type Shares = HList![
        Share<Root, Constructors<HList![Group<MakeRoot>]>>,
        Share<Leaf, Constructors<HList![One<Before>, Group<Both>, One<After>]>>
    ];
}

#[derive(Numbered)]
struct Single;
impl<L> Part<L> for Single {
    type Args = ();
    type Shares = HList![Share<Root, HList![Factory]>, Share<Leaf, HList![Primary]>];
}

struct Grouped;
composition!(impl for Grouped: Root, HList![Mixed], TestClock, u8);
crate::explain!(Grouped, Leaf, build Both of Mixed);
struct Ungrouped;
composition!(impl for Ungrouped: Root, HList![Single], TestClock, u8);

type RootRequests = HList![Requests<NodeRequest<Root, Root>>];

fn leaf(key: u32) -> KeyAddress<Root, Leaf> {
    KeyAddress::<Root, Root>::child::<Leaf, _>(key)
}

#[test]
fn a_group_builds_once_per_added_key_and_its_members_lend_separately() {
    let mut driver = Driver::<Grouped>::new(clock(), (), hlist![()], 0);
    driver
        .operate::<Root, RootRequests, _, _, _>(&KeyAddress::here(), |hlist_pat![requests]| {
            for change in [
                Change::Add(1, Start::new(11, 12)),
                Change::Add(1, Start::new(91, 92)),
                Change::Add(2, Start::new(21, 22)),
            ] {
                requests.push(NodeRequest::new(KeyAddress::here(), change));
            }
        })
        .unwrap()
        .unwrap();
    driver
        .operate::<Root, HList![Ref<Factory>, Ref<Ready>], _, _, _>(
            &KeyAddress::here(),
            |hlist_pat![factory, _]| assert_eq!(factory.built, [1, 2]),
        )
        .unwrap()
        .unwrap();

    for (key, expected) in [(1, (11, 12)), (2, (21, 22))] {
        let actual = driver
            .operate::<Leaf, HList![Ref<Primary>, Ref<Secondary>], _, _, _>(
                &leaf(key),
                |hlist_pat![primary, secondary]| (primary.token.id, secondary.0.id),
            )
            .unwrap()
            .unwrap();
        assert_eq!(actual, expected);
    }

    let wakes = driver
        .operate::<Leaf, HList![WakerOf<Primary>, Wakes<u8>], _, _, _>(
            &leaf(1),
            |hlist_pat![primary, wakes]| wakes.wake(&primary, 0),
        )
        .unwrap();
    assert_eq!(wakes, Some(()));
    assert!(super::poll(&mut driver, Waker::noop()).is_pending());
    for (key, expected) in [(1, 1), (2, 0)] {
        let actual = driver
            .operate::<Leaf, HList![Ref<Primary>], _, _, _>(&leaf(key), |hlist_pat![primary]| {
                primary.wakes
            })
            .unwrap();
        assert_eq!(actual, Some(expected));
    }

    driver
        .operate::<Root, RootRequests, _, _, _>(&KeyAddress::here(), |hlist_pat![requests]| {
            requests.push(NodeRequest::new(KeyAddress::here(), Change::End(1, 7)));
        })
        .unwrap()
        .unwrap();
    driver
        .operate::<Root, HList![Ref<Factory>], _, _, _>(
            &KeyAddress::here(),
            |hlist_pat![factory]| assert_eq!(factory.ended, [(11, 7)]),
        )
        .unwrap()
        .unwrap();
}

#[test]
fn a_group_member_uses_the_same_operations_when_built_on_its_own() {
    let mut driver = Driver::<Ungrouped>::new(clock(), (), hlist![()], 0);
    driver
        .operate::<Root, RootRequests, _, _, _>(&KeyAddress::here(), |hlist_pat![requests]| {
            requests.push(NodeRequest::new(
                KeyAddress::here(),
                Change::Add(1, Start::new(11, 12)),
            ));
        })
        .unwrap()
        .unwrap();
    driver
        .operate::<Leaf, HList![WakerOf<Primary>, Wakes<u8>], _, _, _>(
            &leaf(1),
            |hlist_pat![primary, wakes]| wakes.wake(&primary, 0),
        )
        .unwrap()
        .unwrap();
    assert!(super::poll(&mut driver, Waker::noop()).is_pending());
    let primary = driver
        .operate::<Leaf, HList![Ref<Primary>], _, _, _>(&leaf(1), |hlist_pat![primary]| {
            (primary.token.id, primary.wakes)
        })
        .unwrap();
    assert_eq!(primary, Some((11, 1)));

    driver
        .operate::<Root, RootRequests, _, _, _>(&KeyAddress::here(), |hlist_pat![requests]| {
            requests.push(NodeRequest::new(KeyAddress::here(), Change::End(1, 7)));
        })
        .unwrap()
        .unwrap();
    driver
        .operate::<Root, HList![Ref<Factory>], _, _, _>(
            &KeyAddress::here(),
            |hlist_pat![factory]| {
                assert!(factory.built.is_empty());
                assert_eq!(factory.ended, [(11, 7)]);
            },
        )
        .unwrap()
        .unwrap();
}
