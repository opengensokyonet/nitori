//! What the parts of the two structures do: their arguments, the
//! operations of their components, and their reaction points and
//! reactions. Every reaction and operation that does something notes it in
//! one log, so that a test can compare what ran, in which order.
extern crate std;

use alloc::{format, string::String, vec::Vec};
use core::cell::RefCell;

use frunk_core::{
    HList,
    hlist::{HCons, HNil},
    hlist_pat,
};

use super::fixtures::*;
use crate::{
    component::{ArgsOf, Component, ComponentView, EndCause, Held, Part, PartList},
    id::Numbered,
    need::{Cap, Members, Mut, OwnKey, Pos, Ref, Requests, Under, View},
    react::{NodeEnding, NodeStarted, Reaction, ReactionPoint},
    structure::NodeKind,
};

std::thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
/// Note `line` in the log.
pub fn log(line: String) {
    LOG.with(|l| l.borrow_mut().push(line));
}
/// What was noted since the last time, leaving the log empty.
pub fn take_log() -> Vec<String> {
    LOG.with(|l| core::mem::take(&mut *l.borrow_mut()))
}

// ------------------------------------------------------------------ arguments

/// The core's arguments: nothing.
pub struct CoreArgs;
/// The loss part's arguments: the window a link starts with.
pub struct LossArgs {
    pub window: u32,
}
/// The watch's arguments: nothing.
pub struct WatchArgs;
impl<L> Part<L> for Core {
    type Args = CoreArgs;
}
impl<L> Part<L> for Loss {
    type Args = LossArgs;
}
impl<L> Part<L> for Watch {
    type Args = WatchArgs;
}
/// What a link core reads of one part's arguments.
pub trait WindowArgs {
    fn window(&self) -> Option<u32> {
        None
    }
}
impl WindowArgs for CoreArgs {}
impl WindowArgs for WatchArgs {}
impl WindowArgs for LossArgs {
    fn window(&self) -> Option<u32> {
        Some(self.window)
    }
}
/// The same of the whole list.
pub trait WindowList {
    fn window(&self) -> Option<u32>;
}
impl WindowList for HNil {
    fn window(&self) -> Option<u32> {
        None
    }
}
impl<A: WindowArgs, T: WindowList> WindowList for HCons<A, T> {
    fn window(&self) -> Option<u32> {
        self.head.window().or(self.tail.window())
    }
}

// ------------------------------------------------------------------ components

macro_rules! from_default {
    ($($c:ty),*) => {$(
        impl<O, L: PartList<L>, K: NodeKind> Component<O, L, K> for $c {
            fn build<P: Pos>(
                _: &mut K::Start,
                _: &ArgsOf<L>,
                _: View<'_, Self::BuildNeeds, P>,
            ) -> Self {
                Self::default()
            }
        }
    )*};
}
from_default!(
    ConnCore,
    Keys,
    Credit,
    LinkLimit,
    SentRecords,
    RecoveryFactory
);

/// A link core starts with the window of the loss part, when the list of
/// parts has it, and counts the key epoch when woken.
impl<O, L, K: NodeKind> Component<O, L, K> for LinkCore
where
    L: PartList<L, Args: WindowList>,
{
    type Needs = HList![Ref<Keys>, Mut<RecoveryFactory>];
    fn build<P: Pos>(_: &mut K::Start, args: &ArgsOf<L>, _: View<'_, Self::BuildNeeds, P>) -> Self {
        LinkCore {
            rx: args.window().unwrap_or(0),
        }
    }
    fn wake<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>) {
        let hlist_pat![me, keys, factory] = view;
        me.rx += keys.epoch;
        factory.made += 1;
    }
}

/// A planner counts from the start of its link, its key and the key epoch,
/// and takes one of the start for the next component; it notes how it
/// ends.
impl<L: PartList<L>> Component<Core, L, LinkPath> for Planner {
    type Needs = HList![Ref<LinkCore>];
    type BuildNeeds = HList![OwnKey<LinkId>, Ref<Keys>, Mut<RecoveryFactory>];
    fn build<P: Pos>(start: &mut u64, _: &ArgsOf<L>, view: View<'_, Self::BuildNeeds, P>) -> Self {
        let hlist_pat![key, keys, factory] = view;
        factory.made += 1;
        let next_pn = *start + 100 * u64::from(key.0) + u64::from(keys.epoch);
        *start += 1;
        Planner { next_pn }
    }
    fn end<P: Pos>(view: ComponentView<'_, Self, Self::Needs, P>, cause: EndCause<'_, u8>) {
        let hlist_pat![me, core] = view;
        log(format!("end {cause:?} pn={} rx={}", me.next_pn, core.rx));
    }
}

/// A recovery's congestion starts with what its link's planner is at.
impl<L: PartList<L>> Component<Loss, L, Recovery> for Congestion {
    type BuildNeeds = HList![Ref<Planner>, OwnKey<()>];
    fn build<P: Pos>(_: &mut (), _: &ArgsOf<L>, view: View<'_, Self::BuildNeeds, P>) -> Self {
        let hlist_pat![planner, ()] = view;
        Congestion {
            in_flight: u32::try_from(planner.next_pn).unwrap_or(u32::MAX),
        }
    }
}

/// An estimator the host chooses: a bare type parameter, so it is held.
pub trait Estimator {
    fn new() -> Self;
}
#[derive(Debug, Default, PartialEq, Eq, Numbered)]
pub struct Rtt {
    pub samples: u32,
}
impl Estimator for Rtt {
    fn new() -> Self {
        Rtt { samples: 1 }
    }
}
impl<X: Estimator + Numbered + 'static, L: PartList<L>, K: NodeKind> Component<Watch, L, K>
    for Held<X>
{
    fn build<P: Pos>(_: &mut K::Start, _: &ArgsOf<L>, _: View<'_, Self::BuildNeeds, P>) -> Self {
        Held(X::new())
    }
}

// ------------------------------------------------------------------ reaction points

/// What a send tells: how many packets.
pub struct SentBatch {
    pub count: u32,
}
/// The core's point: packets were sent.
#[derive(Numbered)]
pub struct PacketsSent;
impl ReactionPoint for PacketsSent {
    type Payload<'a> = &'a SentBatch;
}
/// The first and second points of a chain.
#[derive(Numbered)]
pub struct P1;
impl ReactionPoint for P1 {
    type Payload<'a> = u32;
}
#[derive(Numbered)]
pub struct P2;
impl ReactionPoint for P2 {
    type Payload<'a> = u64;
}
/// A point only the watch reacts to, which no test fires.
#[derive(Numbered)]
pub struct Unrelated;
impl ReactionPoint for Unrelated {
    type Payload<'a> = ();
}
/// A point the core and the loss part react to at the connection and at
/// the link path.
#[derive(Numbered)]
pub struct Probe;
impl ReactionPoint for Probe {
    type Payload<'a> = u32;
}
/// A point whose reaction, at a link path, puts in a ping for it.
#[derive(Numbered)]
pub struct Pinged;
impl ReactionPoint for Pinged {
    type Payload<'a> = ();
}

// ------------------------------------------------------------------ reactions

/// The core, at a link path, to packets sent: the link counts them, and so
/// does its recovery's congestion, where there is one.
pub struct RA1;
impl Reaction<PacketsSent> for RA1 {
    type Needs = HList![Mut<LinkCore>, Under<LinkCore, Mut<Congestion>>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, payload: &SentBatch) {
        let hlist_pat![link, mut congestion] = view;
        link.rx += payload.count;
        for (_, c) in congestion.iter_mut() {
            c.in_flight += payload.count;
        }
        log(format!(
            "RA1 count={} congestion={}",
            payload.count,
            congestion.len()
        ));
    }
}
/// The loss part, at the connection, to packets sent: it reads the key
/// epoch and the sent records.
pub struct RA2;
impl Reaction<PacketsSent> for RA2 {
    type Needs = HList![Ref<Keys>, Ref<SentRecords>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, payload: &SentBatch) {
        let hlist_pat![keys, sent] = view;
        log(format!(
            "RA2 epoch={} sent={} count={}",
            keys.epoch,
            sent.sent.len(),
            payload.count
        ));
    }
}
/// The watch's reaction to the point no test fires.
pub struct RZ;
impl Reaction<Unrelated> for RZ {
    type Needs = HList![Mut<ConnCore>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![core] = view;
        core.rx += 1;
        log("RZ".into());
    }
}
/// The core, at a link path, to the first point: it fires the second.
pub struct RC1;
impl Reaction<P1> for RC1 {
    type Needs = HList![Cap<P2>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, payload: u32) {
        let hlist_pat![mut second] = view;
        log(format!("RC1 {payload}"));
        second.fire(u64::from(payload) * 10);
    }
}
/// The loss part, at the connection, to the second point: the planner of
/// the lend point moves on.
pub struct RC2;
impl Reaction<P2> for RC2 {
    type Needs = HList![Mut<Planner>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, payload: u64) {
        let hlist_pat![planner] = view;
        planner.next_pn += payload;
        log(format!("RC2 {payload}"));
    }
}

/// A reaction to the probe that notes `$name` and what it takes.
macro_rules! probe_reaction {
    ($r:ident, $name:literal, $need:ty, |$v:ident| $get:expr) => {
        pub struct $r;
        impl Reaction<Probe> for $r {
            type Needs = HList![$need];
            fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, payload: u32) {
                let hlist_pat![$v] = view;
                let got = $get;
                log(format!("{} {payload} {got}", $name));
            }
        }
    };
}
probe_reaction!(CoreAtConn, "core@conn", Mut<ConnCore>, |c| {
    c.rx += 1;
    c.rx
});
probe_reaction!(CoreAtLink, "core@link", Mut<LinkCore>, |l| {
    l.rx += 1;
    l.rx
});
probe_reaction!(LossAtConn, "loss@conn", Mut<ConnCore>, |c| {
    c.rx += 10;
    c.rx
});
probe_reaction!(LossAtLink, "loss@link", Ref<LinkCore>, |l| l.rx);

/// The core, at the connection, to a link path added: the connection
/// counts it.
pub struct StartedAtConn;
impl Reaction<NodeStarted<LinkPath>> for StartedAtConn {
    type Needs = HList![Mut<ConnCore>, OwnKey<LinkId>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![core, key] = view;
        core.rx += 1;
        log(format!("started@conn {}", key.0));
    }
}
/// The core, at the link path itself, to it being added.
pub struct StartedAtLink;
impl Reaction<NodeStarted<LinkPath>> for StartedAtLink {
    type Needs = HList![Ref<Planner>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![planner] = view;
        log(format!("started@link {}", planner.next_pn));
    }
}
/// The core, at the connection, to a link path ending.
pub struct EndingAtConn;
impl Reaction<NodeEnding<LinkPath>> for EndingAtConn {
    type Needs = HList![Ref<LinkCore>, Mut<ConnCore>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![link, core] = view;
        core.rx += 100;
        log(format!("ending@conn {}", link.rx));
    }
}
/// The core, at the link path itself, to it ending: not taken.
pub struct EndingAtLink;
impl Reaction<NodeEnding<LinkPath>> for EndingAtLink {
    type Needs = HList![];
    fn run<Q: Pos>(_: View<'_, Self::Needs, Q>, _: ()) {
        log("ending@link".into());
    }
}
/// The core, at a link path, to being pinged: it puts in a ping for
/// itself.
pub struct PingIt;
impl Reaction<Pinged> for PingIt {
    type Needs = HList![OwnKey<LinkId>, Requests<Ping>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let hlist_pat![key, pings] = view;
        pings.push(Ping(key));
    }
}
