//! A small structure the driver's tests run: a top node and leaves by
//! number, each leaf with a component that notes its wakes and its end,
//! and a call that follows a script, one step per poll.
extern crate std;

use alloc::{collections::VecDeque, format, string::String, vec::Vec};
use core::{future::poll_fn, task::Poll};
use std::{cell::RefCell, task::Waker};

use frunk_core::{HList, hlist_pat};
use nitori_call::{LentAccess, call};

use crate::{
    call::{CallFamily, CallKind, Composition, LoanHost, LoanNeeds, RootOf},
    component::{ArgsOf, Build, Component, ComponentView, EndCause, Part, PartList},
    composition,
    driver::{Edit, HandlerFor},
    id::Numbered,
    lend::tests::fixtures::TestClock,
    need::{Members, Mut, OwnAddress, OwnKey, Pos, Ref, Requests, Under, View, WakerOf, Wakes},
    request::{HandleRequest, NodeRequest},
    structure::{Child, KeyAddress, NodeKind, Share, SlotVec},
};

std::thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static WAKERS: RefCell<Vec<(u32, Waker)>> = const { RefCell::new(Vec::new()) };
}
/// Note `line`.
pub fn note(line: String) {
    LOG.with(|l| l.borrow_mut().push(line));
}
/// What was noted since the last time, leaving nothing.
pub fn notes() -> Vec<String> {
    LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
}
/// The wakers the calls of the leaf `key` handed out, the last first,
/// leaving none of them.
pub fn take_wakers(key: u32) -> Vec<Waker> {
    WAKERS.with(|s| {
        let mut s = s.borrow_mut();
        let mut out = Vec::new();
        s.retain(|(k, w)| {
            if *k == key {
                out.push(w.clone());
                false
            } else {
                true
            }
        });
        out.reverse();
        out
    })
}

/// The priorities of the tests.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Low,
    Mid,
    High,
}

/// The root: leaves by number; it is asked to change its subtree.
#[derive(Numbered)]
pub struct Top;
impl NodeKind for Top {
    type Children = HList![Child<Leaf, SlotVec<u32>>];
    type Request = TopAsk;
    type Handler = TopHandler;
}
/// A leaf, added with its script, with twigs by number; it ends and can
/// be asked for a code.
#[derive(Numbered)]
pub struct Leaf;
impl NodeKind for Leaf {
    type Children = HList![Child<Twig, SlotVec<u32>>];
    type Start = Vec<Step>;
    type EndReason = u8;
    type Request = u8;
    type Handler = LeafHandler;
}
/// A twig below a leaf, added with whether its call ends it.
#[derive(Numbered)]
pub struct Twig;
impl NodeKind for Twig {
    type Children = HList![];
    type Start = bool;
    type Request = EndMe;
    type Handler = TwigHandler;
}

/// What the top is asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TopAsk {
    /// Note the code.
    Note(u8),
    /// Add the leaf of this number with this script, woken at this level.
    Add(u32, Vec<Step>, Level),
    /// End the leaf of this number for this code.
    End(u32, u8),
    /// Replace the leaf of this number, ended for this code.
    Replace(u32, Vec<Step>, u8, Level),
    /// Add below the leaf the twig of this number, which ends itself or
    /// not.
    AddTwig(u32, u32, bool),
    /// Move the twig from the leaf to the leaf.
    Move { twig: u32, from: u32, to: u32 },
    /// End the top.
    EndSelf,
    /// Panic.
    Panic,
}
/// What a twig is asked: to end.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EndMe;

/// The top's handler: it notes what it is asked and what came of it.
pub struct TopHandler;
impl HandleRequest<Top, Level> for TopHandler {
    type Needs = HList![Ref<Hub>];
    fn handle<P: HandlerFor<Top, Self, Level>>(mut edit: Edit<'_, Top, P>, request: TopAsk) {
        edit.lend(|hlist_pat![_hub]| note(format!("top {request:?}")));
        let out = match request {
            TopAsk::Note(_) => return,
            TopAsk::Add(k, script, p) => format!("{:?}", edit.add::<Leaf, _>(k, script, p)),
            TopAsk::End(k, code) => format!("{:?}", edit.end::<Leaf, _>(&k, code)),
            TopAsk::Replace(k, script, code, p) => {
                format!("{:?}", edit.replace::<Leaf, _>(k, script, code, p))
            }
            TopAsk::AddTwig(leaf, twig, tidy) => match edit.child::<Leaf, _>(&leaf) {
                None => String::from("no leaf"),
                Some(mut child) => {
                    format!("{:?}", child.add::<Twig, _>(twig, tidy, Level::Low))
                }
            },
            TopAsk::Move { twig, from, to } => format!(
                "{:?}",
                edit.move_grandchild::<Leaf, _, Twig, _>(&from, &twig, &to)
            ),
            TopAsk::EndSelf => {
                edit.end_self(());
                return;
            }
            TopAsk::Panic => panic!("the top panics"),
        };
        note(format!("top: {out}"));
    }
}
/// A leaf's handler: it notes the code; from 100 it puts in the code less
/// 100 for itself and the code for the top; from 200 it ends.
pub struct LeafHandler;
impl HandleRequest<Leaf, Level> for LeafHandler {
    type Needs = HList![
        Mut<Ctl>,
        Requests<NodeRequest<Top, Leaf>>,
        Requests<NodeRequest<Top, Top>>,
    ];
    fn handle<P: HandlerFor<Leaf, Self, Level>>(mut edit: Edit<'_, Leaf, P>, code: u8) {
        edit.lend(|hlist_pat![ctl, leaves, tops]| {
            note(format!("leaf {} asked {code}", ctl.key));
            if (100..200).contains(&code) {
                leaves.push(NodeRequest::new(
                    KeyAddress::<Top, Top>::child::<Leaf, _>(ctl.key),
                    code - 100,
                ));
                tops.push(NodeRequest::new(KeyAddress::here(), TopAsk::Note(code)));
            }
        });
        if code >= 200 {
            edit.end_self(code);
        }
    }
}
/// A twig's handler: it ends the twig.
pub struct TwigHandler;
impl<Pr> HandleRequest<Twig, Pr> for TwigHandler {
    type Needs = HList![];
    fn handle<P: HandlerFor<Twig, Self, Pr>>(edit: Edit<'_, Twig, P>, _: EndMe) {
        edit.end_self(());
    }
}

/// A request for a node, as a step puts it in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ask {
    /// For the leaf of this number, with this code.
    Leaf(u32, u8),
    /// For the top.
    Top(TopAsk),
}

/// One poll of a leaf's call.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    /// Wait, woken by nobody.
    Pend,
    /// Wake its leaf's component at each of these priorities, then wait.
    Wake(Vec<Level>),
    /// Wake its own waker, then wait.
    WakeSelf,
    /// Hand out its waker at this priority, then wait.
    Hand(Level),
    /// Put these requests in, then wait.
    Ask(Vec<Ask>),
    /// Put this request in and settle at this priority.
    Settle(Ask, Level),
    /// Like `Settle`, noting the keys of the twigs its leaf has, before
    /// and after the settling.
    SettleSee(Ask, Level),
    /// Wake its own waker on every poll from now on.
    Spin,
    /// Panic.
    Panic,
    /// Keep a guard in the call, which notes its drop, then wait.
    Guard,
    /// Not a step of the call, which waits: its leaf's component panics
    /// when woken.
    PanicOnWake,
}

/// A leaf's component: its number, the rest of its call's script, and how
/// many more times it wakes itself when woken.
#[derive(Debug, Numbered)]
pub struct Ctl {
    pub key: u32,
    pub script: VecDeque<Step>,
    pub polls: u32,
    pub again: u32,
    /// Whether its wake panics.
    pub boom: bool,
}
/// The top's component: it notes its end.
#[derive(Debug, Default, Numbered)]
pub struct Hub;

/// The one part.
#[derive(Numbered)]
pub struct Run;
impl<L> Part<L> for Run {
    type Args = ();
    type Shares = HList![
        Share<Top, HList![Hub]>,
        Share<Leaf, HList![Ctl], HList![Script]>,
        Share<Twig, HList![Tip], HList![Tidy]>,
    ];
}

/// A twig's component: its number, and whether its call ends it.
#[derive(Debug, Numbered)]
pub struct Tip {
    pub key: u32,
    pub tidy: bool,
}
impl<L: PartList<L>> Build<Run, L, Twig> for Tip {
    type BuildNeeds = HList![OwnKey<u32>];
    fn build<P: Pos>(
        tidy: &mut bool,
        _: &(),
        _: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self {
        let hlist_pat![key] = view;
        Tip { key, tidy: *tidy }
    }
}

impl<L: PartList<L>> Component<Run, L, Twig> for Tip {
    type Needs = HList![Ref<Ctl>];
    fn wake<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>) {
        let hlist_pat![me, leaf] = view;
        note(format!("tip {} under {}", me.key, leaf.key));
    }
    fn end<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>, cause: EndCause<'_, ()>) {
        let hlist_pat![me, _] = view;
        note(format!("end twig {} {cause:?}", me.key));
    }
}

/// The call of a twig, written for any structure and any kind that starts
/// from whether it ends itself and is asked to end: when its component
/// says so, it asks its own node to end, by its own address.
impl<Q: Composition, K: NodeKind<Start = bool, Request = EndMe>> CallKind<Q, K> for Tidy {
    type Needs = HList![
        Ref<Tip>,
        OwnAddress<KeyAddress<RootOf<Q>, K>>,
        OwnKey<u32>,
        Requests<NodeRequest<RootOf<Q>, K>>,
    ];
    type Op = Self;
    fn start(_: &mut bool) -> Self {
        Tidy::new()
    }
}
#[call]
pub async fn tidy<W: CallFamily<C = Tidy, K: NodeKind<Start = bool, Request = EndMe>>>(
    host: &mut impl LentAccess<Family = W>,
) {
    host.with(|mut view| {
        let hlist_pat![tip, own, key, requests] = view.needs();
        if tip.tidy {
            note(format!("tidy {key}"));
            requests.push(NodeRequest::new(own.clone(), EndMe));
        }
    });
}

impl<L: PartList<L>> Build<Run, L, Top> for Hub {
    fn build<P: Pos>(_: &mut (), _: &(), _: &ArgsOf<L>, _: View<'_, Self::BuildNeeds, P>) -> Self {
        Hub
    }
}

impl<L: PartList<L>> Component<Run, L, Top> for Hub {
    fn end<P: Pos>(_: ComponentView<'_, Self, Self::Needs, P>, cause: EndCause<'_, ()>) {
        note(format!("end top {cause:?}"));
    }
}
impl<L: PartList<L>> Build<Run, L, Leaf> for Ctl {
    type BuildNeeds = HList![OwnKey<u32>];
    fn build<P: Pos>(
        start: &mut Vec<Step>,
        _: &(),
        _: &ArgsOf<L>,
        view: View<'_, Self::BuildNeeds, P>,
    ) -> Self {
        let hlist_pat![key] = view;
        let again = start
            .iter()
            .filter(|s| **s == Step::WakeSelf)
            .count()
            .try_into()
            .unwrap_or(0);
        let boom = start.contains(&Step::PanicOnWake);
        Ctl {
            key,
            script: core::mem::take(start).into(),
            polls: 0,
            again,
            boom,
        }
    }
}

impl<L: PartList<L>> Component<Run, L, Leaf> for Ctl {
    type Needs = HList![WakerOf<Ctl>, Wakes<Level>];
    fn wake<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>) {
        let hlist_pat![me, own, wakes] = view;
        note(format!("ctl {}", me.key));
        if me.boom {
            panic!("the component panics");
        }
        if me.again > 0 {
            me.again -= 1;
            wakes.wake(&own, Level::Low);
        }
    }
    fn end<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>, cause: EndCause<'_, u8>) {
        let hlist_pat![me, _, _] = view;
        note(format!("end {} {cause:?}", me.key));
    }
}

/// The call of a leaf: one step of its script per poll.
impl<Q: Composition<Priority = Level>> CallKind<Q, Leaf> for Script {
    type Needs = HList![
        crate::need::Mut<Ctl>,
        WakerOf<Ctl>,
        Wakes<Level>,
        Requests<NodeRequest<Top, Leaf>>,
        Requests<NodeRequest<Top, Top>>,
        Under<Ctl, Ref<Tip>>,
    ];
    type Op = Self;
    fn start(_: &mut Vec<Step>) -> Self {
        Script::new()
    }
}

/// What a step leaves the call to do after the lending.
enum Then {
    Wait,
    WakeSelf,
    Hand,
    Settle(crate::call::Settle),
    SettleSee(crate::call::Settle),
    Spin,
    Guard,
}

/// Notes its drop: a call is dropped before its node's components end.
struct Guard(u32);
impl Drop for Guard {
    fn drop(&mut self) {
        note(format!("drop call {}", self.0));
    }
}

/// The keys of the twigs below the leaf, in the order the view holds them.
fn twig_keys<'a, V: Members<&'a Tip>>(twigs: &V) -> Vec<u32> {
    twigs.iter().map(|(_, t)| t.key).collect()
}

/// The address of what `ask` is for, and the request.
fn put(
    ask: Ask,
    leaves: crate::need::RequestBox<'_, NodeRequest<Top, Leaf>>,
    tops: crate::need::RequestBox<'_, NodeRequest<Top, Top>>,
) {
    match ask {
        Ask::Leaf(k, code) => leaves.push(NodeRequest::new(
            KeyAddress::<Top, Top>::child::<Leaf, _>(k),
            code,
        )),
        Ask::Top(code) => tops.push(NodeRequest::new(KeyAddress::here(), code)),
    }
}

#[call]
pub async fn script<W: CallFamily<C = Script, K = Leaf, Q: Composition<Priority = Level>>>(
    host: &mut impl LentAccess<Family = W>,
) {
    let mut spinning = false;
    let mut guards = Vec::new();
    loop {
        let (then, key) = host.with(|mut view| {
            let (step, key) = {
                let hlist_pat![ctl, _, _, _, _, _] = view.needs();
                ctl.polls += 1;
                note(format!("call {} #{}", ctl.key, ctl.polls));
                let step = if spinning {
                    Step::Spin
                } else {
                    ctl.script.pop_front().unwrap_or(Step::Pend)
                };
                (step, ctl.key)
            };
            let then = match step {
                Step::Pend => Then::Wait,
                Step::Wake(levels) => {
                    let hlist_pat![_, own, wakes, _, _, _] = view.needs();
                    for p in levels {
                        wakes.wake(own, p);
                    }
                    Then::Wait
                }
                Step::WakeSelf => Then::WakeSelf,
                Step::Hand(p) => {
                    view.set_wake_priority(p);
                    Then::Hand
                }
                Step::Ask(asks) => {
                    let hlist_pat![_, _, _, leaves, tops, _] = view.needs();
                    for a in asks {
                        put(a, *leaves, *tops);
                    }
                    Then::Wait
                }
                Step::Settle(ask, p) => {
                    {
                        let hlist_pat![_, _, _, leaves, tops, _] = view.needs();
                        put(ask, *leaves, *tops);
                    }
                    Then::Settle(view.settle(p))
                }
                Step::SettleSee(ask, p) => {
                    let hlist_pat![_, _, _, leaves, tops, twigs] = view.needs();
                    note(format!("call {key} sees twigs {:?}", twig_keys(&*twigs)));
                    put(ask, *leaves, *tops);
                    Then::SettleSee(view.settle(p))
                }
                Step::Spin => Then::Spin,
                Step::Panic => panic!("the script panics"),
                Step::Guard => Then::Guard,
                Step::PanicOnWake => Then::Wait,
            };
            (then, key)
        });
        match then {
            Then::Settle(s) => {
                s.await;
                note(format!("call {key} settled"));
            }
            Then::SettleSee(s) => {
                s.await;
                host.with(|mut view| {
                    let hlist_pat![_, _, _, _, _, twigs] = view.needs();
                    note(format!("call {key} sees twigs {:?}", twig_keys(&*twigs)));
                });
                note(format!("call {key} settled"));
            }
            Then::Spin | Then::WakeSelf => {
                spinning |= matches!(then, Then::Spin);
                poll_fn(|cx| {
                    cx.waker().wake_by_ref();
                    Poll::Ready(())
                })
                .await;
                once_pending().await;
            }
            Then::Hand => {
                poll_fn(|cx| {
                    WAKERS.with(|s| s.borrow_mut().push((key, cx.waker().clone())));
                    Poll::Ready(())
                })
                .await;
                once_pending().await;
            }
            Then::Guard => {
                guards.push(Guard(key));
                once_pending().await;
            }
            Then::Wait => once_pending().await,
        }
    }
}

/// Wait once: pending on the first poll, ready on the next.
async fn once_pending() {
    let mut first = true;
    poll_fn(|_| {
        if core::mem::take(&mut first) {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
}

/// The structure composed.
pub struct QT;
composition!(impl for QT: Top, HList![Run], TestClock, Level);

/// A clock for the tests.
pub fn clock() -> TestClock {
    TestClock {
        last: core::time::Duration::from_secs(10),
    }
}
