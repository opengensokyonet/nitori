//! The structure the examples of the documentation lend from, `Conn {
//! ConnCore, Label; [LogicalPath by PathKey { SentRecords; [LinkPath by
//! LinkId { LinkCore, Planner, Label; Recovery? { Congestion } }] }] }`,
//! all of it in the one part `V1`; the requests `Ping` and `Close`; the
//! capability [`Planning`] and the reaction points [`Sent`] and [`Echo`];
//! and [`lends!`], [`lends_in!`] and [`component_lends!`], which prove
//! needs on it.
use crate::{
    capability::Capability,
    frunk_core::{HList, hlist_pat},
    id::Numbered,
    need::{Cap, Mut, Pos, Ref, View},
    react::{Reaction, ReactionPoint, ReactsTo},
    structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
};

/// The connection's core.
#[derive(Numbered)]
pub struct ConnCore;
/// A logical path's sent records.
#[derive(Numbered)]
pub struct SentRecords;
/// A link's core.
#[derive(Numbered)]
pub struct LinkCore;
/// A link's planner.
#[derive(Numbered)]
pub struct Planner;
/// A recovery's congestion.
#[derive(Numbered)]
pub struct Congestion;
/// A label, of the connection and of every link.
#[derive(Numbered)]
pub struct Label;
/// The key of a logical path.
#[derive(Clone, PartialEq, Eq)]
pub struct PathKey(pub u32);
/// The key of a link.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkId(pub u32);

/// The connection, the root.
pub struct Conn;
impl NodeKind for Conn {
    type Children = HList![Child<LogicalPath, SlotVec<PathKey>>];
}
/// A logical path.
pub struct LogicalPath;
impl NodeKind for LogicalPath {
    type Children = HList![Child<LinkPath, SlotVec<LinkId>>];
}
/// A link path.
pub struct LinkPath;
impl NodeKind for LinkPath {
    type Children = HList![Child<Recovery, AtMostOne>];
}
/// A link path's recovery.
pub struct Recovery;
impl NodeKind for Recovery {
    type Children = HList![];
}

/// The one part.
pub struct V1;
impl Share<Conn> for V1 {
    type Data = HList![ConnCore, Label];
    type Reacts = HList![ReactsTo<Echo, Repeat>];
}
impl Share<LogicalPath> for V1 {
    type Data = HList![SentRecords];
}
impl Share<LinkPath> for V1 {
    type Data = HList![LinkCore, Planner, Label];
    type Reacts = HList![ReactsTo<Sent, CountSent>];
}
impl Share<Recovery> for V1 {
    type Data = HList![Congestion];
}
/// The parts.
pub type Parts = HList![V1];

/// Planning a packet: a link's planner, exclusive, and its core.
pub struct Planning;
/// What [`Planning`] is lent as.
pub struct Plan<'t> {
    /// The planner.
    pub planner: &'t mut Planner,
    /// The link's core.
    pub core: &'t LinkCore,
}
impl Capability for Planning {
    type Needs = HList![Mut<Planner>, Ref<LinkCore>];
    type Object<'t, P: Pos> = Plan<'t>;
    fn assemble<'t, P: Pos>(view: View<'t, Self::Needs, P>) -> Plan<'t> {
        let hlist_pat![planner, core] = view;
        Plan { planner, core }
    }
}

/// A packet was sent on a link, with its number.
#[derive(Numbered)]
pub struct Sent;
impl ReactionPoint for Sent {
    type Payload<'a> = u64;
}
/// The link's reaction to a packet sent: its core, exclusive.
pub struct CountSent;
impl Reaction<Sent> for CountSent {
    type Needs = HList![Mut<LinkCore>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: u64) {
        let _ = view;
    }
}
/// A point whose reaction, at the connection, fires it again.
#[derive(Numbered)]
pub struct Echo;
impl ReactionPoint for Echo {
    type Payload<'a> = ();
}
/// The reaction to [`Echo`]: firing it again.
pub struct Repeat;
impl Reaction<Echo> for Repeat {
    type Needs = HList![Cap<Echo>];
    fn run<Q: Pos>(view: View<'_, Self::Needs, Q>, _: ()) {
        let _ = view;
    }
}

/// A request to probe a link.
pub struct Ping;
/// A request to close the connection.
pub struct Close;

/// Prove the needs `$n` at the kind `$l` of [`Conn`], with no context.
#[doc(hidden)]
#[macro_export]
macro_rules! __doc_lends {
    ($l:ty, $n:ty) => {
        $crate::__private::assert_lends::<
            $crate::__private::doc::Conn,
            $crate::__private::doc::Parts,
            $l,
            $n,
            _,
            _,
        >()
    };
}
/// Prove the needs `$n` at the kind `$l` of [`Conn`], lent with a
/// context of the pieces `$x`.
#[doc(hidden)]
#[macro_export]
macro_rules! __doc_lends_in {
    ($x:ty, $l:ty, $n:ty) => {
        $crate::__private::assert_lends_in::<
            $crate::__private::doc::Conn,
            $crate::__private::doc::Parts,
            $crate::__private::doc::Context<$x>,
            $l,
            $n,
            _,
            _,
        >()
    };
}
/// Prove what the component `$c` at the kind `$l` of [`Conn`] is lent
/// with the needs `$n`: itself, then `$n`.
#[doc(hidden)]
#[macro_export]
macro_rules! __doc_component_lends {
    ($l:ty, $c:ty, $n:ty) => {
        $crate::__private::assert_component_lends::<
            $crate::__private::doc::Conn,
            $crate::__private::doc::Parts,
            $l,
            $c,
            $n,
            _,
            _,
        >()
    };
}
pub use crate::{
    __doc_component_lends as component_lends, __doc_lends as lends, __doc_lends_in as lends_in,
    lend::context::{Context, RequestList},
};
