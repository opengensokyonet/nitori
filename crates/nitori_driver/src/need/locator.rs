//! Custom locating: a lend point picked among candidates by a rule of its
//! own, under a contract its author keeps.
use super::{Need, Pos, View};

/// A rule that picks one node among candidates, the new lend point of an
/// [`At`](super::At)`<Self, T>`.
///
/// The candidates ([`Among`](Self::Among)) are `Under<A, X>`: the nodes of
/// the one kind, at or below the anchor, that has the component `X`, the
/// anchor being the one node from the lend point up to the root that has
/// `A`; `X` is not taken. The rule reads its [`Input`](Self::Input), lent
/// at the lend point, and the candidates' keys, and gives the key of the
/// one it picks ([`pick`](Self::pick)).
///
/// The [`Count`](Self::Count) is a contract the author of the rule keeps,
/// which the driver does not prove: with [`OneOrNone`] the view of the
/// `At` is an `Option`, with [`ExactlyOne`] the view itself, and a rule
/// that picks no member then is a broken invariant: the lending panics,
/// naming the locator. What `T` takes overlaps other needs as anywhere
/// else, by kind and component; the input only reads.
///
/// From a link path, its logical path's link with the next id:
///
/// ```
/// # use nitori_driver::{
/// #     frunk_core::HList,
/// #     id::Numbered,
/// #     need::*,
/// #     structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
/// # };
/// # #[derive(Numbered)] struct ConnCore;
/// # #[derive(Numbered)] struct SentRecords;
/// # #[derive(Numbered)] struct LinkCore;
/// # #[derive(Numbered)] struct Planner;
/// # #[derive(Numbered)] struct Congestion;
/// # #[derive(Numbered)] struct Label;
/// # #[derive(Clone, PartialEq, Eq)] struct PathKey(u32);
/// # #[derive(Clone, PartialEq, Eq)] struct LinkId(u32);
/// # struct Conn;
/// # impl NodeKind for Conn { type Children = HList![Child<LogicalPath, SlotVec<PathKey>>]; }
/// # struct LogicalPath;
/// # impl NodeKind for LogicalPath { type Children = HList![Child<LinkPath, SlotVec<LinkId>>]; }
/// # struct LinkPath;
/// # impl NodeKind for LinkPath { type Children = HList![Child<Recovery, AtMostOne>]; }
/// # struct Recovery;
/// # impl NodeKind for Recovery { type Children = HList![]; }
/// # struct V1;
/// # impl Share<Conn> for V1 { type Data = HList![ConnCore, Label]; }
/// # impl Share<LogicalPath> for V1 { type Data = HList![SentRecords]; }
/// # impl Share<LinkPath> for V1 { type Data = HList![LinkCore, Planner, Label]; }
/// # impl Share<Recovery> for V1 { type Data = HList![Congestion]; }
/// # macro_rules! lends {
/// #     ($l:ty, $n:ty) => {
/// #         nitori_driver::__private::assert_lends::<Conn, HList![V1], $l, $n, _, _>()
/// #     };
/// # }
/// struct NextLink;
/// impl Locator for NextLink {
///     type Among = Under<SentRecords, LinkCore>;
///     type Input = OwnKey<LinkId>;
///     type Count = OneOrNone;
///     type Key = LinkId;
///     fn pick<'t, P: Pos>(own: View<'t, OwnKey<LinkId>, P>, among: &[LinkId]) -> Option<LinkId> {
///         among.iter().find(|k| k.0 == own.0 + 1).cloned()
///     }
/// }
/// lends!(LinkPath, HList![Mut<Planner>, At<NextLink, HList![Mut<LinkCore>]>]);
/// ```
///
/// An input that takes exclusive does not compile:
///
/// ```compile_fail,E0277
/// # use nitori_driver::{
/// #     frunk_core::HList,
/// #     id::Numbered,
/// #     need::*,
/// #     structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
/// # };
/// # #[derive(Numbered)] struct ConnCore;
/// # #[derive(Numbered)] struct SentRecords;
/// # #[derive(Numbered)] struct LinkCore;
/// # #[derive(Numbered)] struct Planner;
/// # #[derive(Numbered)] struct Congestion;
/// # #[derive(Numbered)] struct Label;
/// # #[derive(Clone, PartialEq, Eq)] struct PathKey(u32);
/// # #[derive(Clone, PartialEq, Eq)] struct LinkId(u32);
/// # struct Conn;
/// # impl NodeKind for Conn { type Children = HList![Child<LogicalPath, SlotVec<PathKey>>]; }
/// # struct LogicalPath;
/// # impl NodeKind for LogicalPath { type Children = HList![Child<LinkPath, SlotVec<LinkId>>]; }
/// # struct LinkPath;
/// # impl NodeKind for LinkPath { type Children = HList![Child<Recovery, AtMostOne>]; }
/// # struct Recovery;
/// # impl NodeKind for Recovery { type Children = HList![]; }
/// # struct V1;
/// # impl Share<Conn> for V1 { type Data = HList![ConnCore, Label]; }
/// # impl Share<LogicalPath> for V1 { type Data = HList![SentRecords]; }
/// # impl Share<LinkPath> for V1 { type Data = HList![LinkCore, Planner, Label]; }
/// # impl Share<Recovery> for V1 { type Data = HList![Congestion]; }
/// # macro_rules! lends {
/// #     ($l:ty, $n:ty) => {
/// #         nitori_driver::__private::assert_lends::<Conn, HList![V1], $l, $n, _, _>()
/// #     };
/// # }
/// struct NextLink;
/// impl Locator for NextLink {
///     type Among = Under<SentRecords, LinkCore>;
///     type Input = Mut<Planner>;
///     type Count = OneOrNone;
///     type Key = LinkId;
///     fn pick<'t, P: Pos>(_: View<'t, Mut<Planner>, P>, among: &[LinkId]) -> Option<LinkId> {
///         among.first().cloned()
///     }
/// }
/// lends!(LinkPath, HList![At<NextLink, HList![Mut<LinkCore>]>]);
/// ```
pub trait Locator: 'static {
    /// The candidates: `Under<A, X>`.
    type Among;
    /// What the rule reads, lent at the lend point; it takes only shared.
    type Input: Need;
    /// The contract: [`ExactlyOne`] or [`OneOrNone`].
    type Count: Count;
    /// What tells the candidates apart: the key of the members of the
    /// `Under` of [`Among`](Self::Among).
    type Key: Clone + PartialEq + 'static;
    /// The key of the candidate picked, from the input's view at the
    /// position `P` and the candidates' keys `among`; `None` when there is
    /// none to pick.
    ///
    /// A key that is none of the candidates' picks none.
    fn pick<'t, P: Pos>(input: View<'t, Self::Input, P>, among: &[Self::Key]) -> Option<Self::Key>;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::ExactlyOne {}
    impl Sealed for super::OneOrNone {}
}

/// The contract of a [`Locator`], and what it makes of the view: one of
/// [`ExactlyOne`] and [`OneOrNone`].
pub trait Count: sealed::Sealed + 'static {
    /// The view of what is taken at the node picked, `V`, under this
    /// contract.
    type Out<V>;
}
/// Exactly one node is picked: the view is that of the node.
pub struct ExactlyOne;
impl Count for ExactlyOne {
    type Out<V> = V;
}
/// At most one node is picked: the view is an `Option`.
pub struct OneOrNone;
impl Count for OneOrNone {
    type Out<V> = Option<V>;
}

/// What an [`At`](super::At) moves the lend point to, and so what its view
/// is: a component, the one node that has it being there ([`ExactlyOne`]),
/// or a [`Locator`], by its contract.
///
/// `#[derive(Numbered)]` implements it for a component; a locator has it
/// from [`Locator`].
pub trait AtTarget {
    /// The contract.
    type Count: Count;
}
impl<L: Locator> AtTarget for L {
    type Count = L::Count;
}
