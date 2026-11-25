//! The driver's two reaction points, fired by the driver at the nodes it
//! adds and ends: `NodeStarted` and `NodeEnding`, in the order of adding
//! and of the ending of a subtree, by `replace`, not by a move, and at the
//! root when it ends in `Driver::new`.
//!
//! A part of its own, `Watch`, reacts and notes; the part `Run` of the
//! driver's fixture is composed beside it, so the notes of its own are the
//! ones the other tests see.
extern crate std;

use alloc::{format, vec, vec::Vec};

use frunk_core::{HList, hlist, hlist_pat};

use super::{
    fixture::{Leaf, Level, Run, Step, Top, TopAsk, Twig, clock, note, notes},
    outer, poll,
};
use crate::{
    component::Part,
    driver::{Driver, OperateError},
    id::Numbered,
    need::{OwnKey, Pos, Requests, View},
    react::{NodeEnding, NodeStarted, Reaction, ReactsTo},
    request::NodeRequest,
    structure::{KeyAddress, Share},
};

/// Notes the nodes added and ending, below the top.
#[derive(Numbered)]
pub struct Watch;
impl<L> Part<L> for Watch {
    type Args = ();
    type Shares = HList![
        Share<
            Top,
            HList![],
            HList![],
            HList![ReactsTo<NodeStarted<Leaf>, LeafStarted>, ReactsTo<NodeEnding<Leaf>, LeafEnding>],
        >,
        Share<
            Leaf,
            HList![],
            HList![],
            HList![ReactsTo<NodeStarted<Twig>, TwigStarted>, ReactsTo<NodeEnding<Twig>, TwigEnding>],
        >,
    ];
}
/// Notes the top added, and ends it.
#[derive(Numbered)]
pub struct Quit;
impl<L> Part<L> for Quit {
    type Args = ();
    type Shares = HList![
        Share<Top, HList![], HList![], HList![ReactsTo<NodeStarted<Top>, EndTopAtStart>]>,
    ];
}

/// At the top, to a leaf added.
pub struct LeafStarted;
impl Reaction<NodeStarted<Leaf>> for LeafStarted {
    type Needs = HList![OwnKey<u32>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![key] = view;
        note(format!("started leaf {key}"));
    }
}
/// At the top, to a leaf ending.
pub struct LeafEnding;
impl Reaction<NodeEnding<Leaf>> for LeafEnding {
    type Needs = HList![OwnKey<u32>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![key] = view;
        note(format!("ending leaf {key}"));
    }
}
/// At a leaf, to a twig added.
pub struct TwigStarted;
impl Reaction<NodeStarted<Twig>> for TwigStarted {
    type Needs = HList![OwnKey<u32>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![key] = view;
        note(format!("started twig {key}"));
    }
}
/// At a leaf, to a twig ending.
pub struct TwigEnding;
impl Reaction<NodeEnding<Twig>> for TwigEnding {
    type Needs = HList![OwnKey<u32>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![key] = view;
        note(format!("ending twig {key}"));
    }
}
/// At the top, to the top added: it asks the top to end.
pub struct EndTopAtStart;
impl Reaction<NodeStarted<Top>> for EndTopAtStart {
    type Needs = HList![Requests<NodeRequest<Top, Top>>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![tops] = view;
        note("started top".into());
        tops.push(NodeRequest::new(KeyAddress::here(), TopAsk::EndSelf));
    }
}

/// The fixture's structure with `Watch` beside its part.
pub struct QW;
crate::composition!(impl for QW: Top, HList![Run, Watch], crate::lend::tests::fixtures::TestClock, Level);
/// The fixture's structure with `Quit` beside its part: the top ends as
/// it starts.
pub struct QE;
crate::composition!(impl for QE: Top, HList![Run, Quit], crate::lend::tests::fixtures::TestClock, Level);

/// A driver with nothing but the top, the notes dropped.
fn watched() -> Driver<QW> {
    let d = Driver::<QW>::new(clock(), (), hlist![(), ()], Level::Low);
    notes();
    d
}
/// Ask the top `asks`, in order, from outside.
fn ask(d: &mut Driver<QW>, asks: Vec<TopAsk>) {
    let out = d.operate::<Top, HList![Requests<NodeRequest<Top, Top>>], _, _, _>(
        &KeyAddress::here(),
        |hlist_pat![tops]| {
            for a in asks {
                tops.push(NodeRequest::new(KeyAddress::here(), a));
            }
        },
    );
    assert_eq!(out, Ok(Some(())));
}

/// a: a node added is told to its reactions once, at the add; its call
/// runs after, on the next poll. (That its calls are woken before the
/// reaction runs is not seen from a reaction, nor from a poll: the wakes
/// a reaction makes are taken after the whole add.)
#[test]
fn a_node_added_fires_started_once() {
    let mut d = watched();
    ask(
        &mut d,
        vec![
            TopAsk::Add(1, vec![Step::Pend], Level::Low),
            TopAsk::AddTwig(1, 10, false),
        ],
    );
    assert_eq!(
        notes(),
        [
            "top Add(1, [Pend], Low)",
            "started leaf 1",
            "top: Ok(())",
            "top AddTwig(1, 10, false)",
            "started twig 10",
            "top: Ok(())",
        ]
    );
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    assert_eq!(notes(), ["call 1 #1"]);
}

/// a: a subtree ends from the nodes below: each node's `NodeEnding`, then
/// its calls dropped, then its components' `end`; the node's own
/// `NodeEnding` is taken by the kinds above it only.
#[test]
fn a_subtree_ends_children_first_each_ending_then_call_then_end() {
    let mut d = watched();
    ask(
        &mut d,
        vec![
            TopAsk::Add(1, vec![Step::Guard], Level::Low),
            TopAsk::AddTwig(1, 10, false),
            TopAsk::AddTwig(1, 11, false),
        ],
    );
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    notes();
    ask(&mut d, vec![TopAsk::End(1, 7)]);
    assert_eq!(
        notes(),
        [
            "top End(1, 7)",
            "ending twig 10",
            "end twig 10 Above",
            "ending twig 11",
            "end twig 11 Above",
            "ending leaf 1",
            "drop call 1",
            "end 1 Reason(7)",
            "top: Ok(())",
        ]
    );
}

/// a: `replace` ends the node, then adds the new one: both fire.
#[test]
fn replace_fires_ending_and_started() {
    let mut d = watched();
    ask(&mut d, vec![TopAsk::Add(1, vec![Step::Guard], Level::Low)]);
    let (_, w) = outer();
    let _ = poll(&mut d, &w);
    notes();
    ask(&mut d, vec![TopAsk::Replace(1, vec![], 5, Level::Low)]);
    assert_eq!(
        notes(),
        [
            "top Replace(1, [], 5, Low)",
            "ending leaf 1",
            "drop call 1",
            "end 1 Reason(5)",
            "started leaf 1",
            "top: Ok(())",
        ]
    );
}

/// a: a move changes neither the node's start nor its end.
#[test]
fn a_move_fires_neither() {
    let mut d = watched();
    ask(
        &mut d,
        vec![
            TopAsk::Add(1, vec![], Level::Low),
            TopAsk::Add(2, vec![], Level::Low),
            TopAsk::AddTwig(1, 10, false),
        ],
    );
    notes();
    ask(
        &mut d,
        vec![TopAsk::Move {
            twig: 10,
            from: 1,
            to: 2,
        }],
    );
    assert_eq!(
        notes(),
        ["top Move { twig: 10, from: 1, to: 2 }", "top: Ok(())"]
    );
}

/// S3: the root ends in `Driver::new` (a reaction of one of its parts to
/// its own start asks it to): the driver is done at once.
#[test]
fn a_root_ended_in_new_is_done_at_once() {
    notes();
    let mut d = Driver::<QE>::new(clock(), (), hlist![(), ()], Level::Low);
    assert_eq!(
        notes(),
        ["started top", "top EndSelf", "end top Reason(())"]
    );
    let out = d.operate::<Top, HList![], _, _, _>(&KeyAddress::here(), |_| ());
    assert_eq!(out, Err(OperateError::Ended));
    let (_, w) = outer();
    assert!(poll(&mut d, &w).is_ready());
    drop(d);
    assert!(notes().is_empty());
}
