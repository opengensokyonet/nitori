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
    call::{CallFamily, CallKind, Composition, LoanHost, LoanNeeds},
    component::{ArgsOf, Component, ComponentView, EndCause, Part, PartList},
    composition,
    id::Numbered,
    lend::tests::fixtures::TestClock,
    need::{OwnKey, Pos, Requests, View, WakerOf, Wakes},
    request::NodeRequest,
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

/// The root: leaves by number; it can be asked for a code.
#[derive(Numbered)]
pub struct Top;
impl NodeKind for Top {
    type Children = HList![Child<Leaf, SlotVec<u32>>];
    type Request = u8;
}
/// A leaf, added with its script; it ends and can be asked for a code.
#[derive(Numbered)]
pub struct Leaf;
impl NodeKind for Leaf {
    type Children = HList![];
    type Start = Vec<Step>;
    type EndReason = u8;
    type Request = u8;
}

/// A request for a node, as a step puts it in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    /// For the leaf of this number, with this code.
    Leaf(u32, u8),
    /// For the top, with this code.
    Top(u8),
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
    /// Wake its own waker on every poll from now on.
    Spin,
    /// Panic.
    Panic,
    /// Return.
    Return,
}

/// A leaf's component: its number, the rest of its call's script, and how
/// many more times it wakes itself when woken.
#[derive(Debug, Numbered)]
pub struct Ctl {
    pub key: u32,
    pub script: VecDeque<Step>,
    pub polls: u32,
    pub again: u32,
}
/// The top's component: it notes its end.
#[derive(Debug, Default, Numbered)]
pub struct Hub;

/// The one part.
pub struct Run;
impl<L> Part<L> for Run {
    type Args = ();
}
impl Share<Top> for Run {
    type Data = HList![Hub];
}
impl Share<Leaf> for Run {
    type Data = HList![Ctl];
    type Calls = HList![Script];
}

impl<L: PartList<L>> Component<Run, L, Top> for Hub {
    fn build<P: Pos>(_: &mut (), _: &ArgsOf<L>, _: View<'_, Self::BuildNeeds, P>) -> Self {
        Hub
    }
    fn end<P: Pos>(_: ComponentView<'_, Self, Self::Needs, P>, cause: EndCause<'_, ()>) {
        note(format!("end top {cause:?}"));
    }
}
impl<L: PartList<L>> Component<Run, L, Leaf> for Ctl {
    type Needs = HList![WakerOf<Ctl>, Wakes<Level>];
    type BuildNeeds = HList![OwnKey<u32>];
    fn build<P: Pos>(
        start: &mut Vec<Step>,
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
        Ctl {
            key,
            script: core::mem::take(start).into(),
            polls: 0,
            again,
        }
    }
    fn wake<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>) {
        let hlist_pat![me, own, wakes] = view;
        note(format!("ctl {}", me.key));
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
    Spin,
    Return,
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
    loop {
        let (then, key) = host.with(|mut view| {
            let (step, key) = {
                let hlist_pat![ctl, _, _, _, _] = view.needs();
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
                    let hlist_pat![_, own, wakes, _, _] = view.needs();
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
                    let hlist_pat![_, _, _, leaves, tops] = view.needs();
                    for a in asks {
                        put(a, *leaves, *tops);
                    }
                    Then::Wait
                }
                Step::Settle(ask, p) => {
                    {
                        let hlist_pat![_, _, _, leaves, tops] = view.needs();
                        put(ask, *leaves, *tops);
                    }
                    Then::Settle(view.settle(p))
                }
                Step::Spin => Then::Spin,
                Step::Panic => panic!("the script panics"),
                Step::Return => Then::Return,
            };
            (then, key)
        });
        match then {
            Then::Return => return,
            Then::Settle(s) => {
                s.await;
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
