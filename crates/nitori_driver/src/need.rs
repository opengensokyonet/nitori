//! The words of a need: what is taken from the tree around a lend point.
//!
//! A need is written in terms of components, never of kinds: [`Mut`] and
//! [`Ref`] take the component on the way up from the lend point to the
//! root, [`Under`] takes it from every node of one kind below an anchor,
//! and [`At`] moves the lend point. [`OwnKey`] and [`OwnAddress`] read the
//! lend point itself. A list of needs is an `HList` of them, taken at once
//! and disjoint.
//!
//! The view of a need ([`Need::View`]) depends on the need and on a
//! position ([`Pos`]) the driver infers where the structure is known, so a
//! call generic over the structure names its view from its needs. Where a
//! need is proved, a component that no node or more than one node has, or
//! two needs that take the same component of the same kind and one of them
//! exclusive, do not compile, and the error names the component.
//!
//! The examples use the structure `Conn { ConnCore, Label; [LogicalPath by
//! PathKey { SentRecords; [LinkPath by LinkId { LinkCore, Planner, Label;
//! Recovery? { Congestion } }] }] }`, lent at a link path:
//!
//! ```
//! # use nitori_driver::{
//! #     frunk_core::HList,
//! #     id::Numbered,
//! #     need::*,
//! #     structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
//! # };
//! # #[derive(Numbered)] struct ConnCore;
//! # #[derive(Numbered)] struct SentRecords;
//! # #[derive(Numbered)] struct LinkCore;
//! # #[derive(Numbered)] struct Planner;
//! # #[derive(Numbered)] struct Congestion;
//! # #[derive(Numbered)] struct Label;
//! # #[derive(Clone, PartialEq, Eq)] struct PathKey(u32);
//! # #[derive(Clone, PartialEq, Eq)] struct LinkId(u32);
//! # struct Conn;
//! # impl NodeKind for Conn { type Children = HList![Child<LogicalPath, SlotVec<PathKey>>]; }
//! # struct LogicalPath;
//! # impl NodeKind for LogicalPath { type Children = HList![Child<LinkPath, SlotVec<LinkId>>]; }
//! # struct LinkPath;
//! # impl NodeKind for LinkPath { type Children = HList![Child<Recovery, AtMostOne>]; }
//! # struct Recovery;
//! # impl NodeKind for Recovery { type Children = HList![]; }
//! # struct V1;
//! # impl Share<Conn> for V1 { type Data = HList![ConnCore, Label]; }
//! # impl Share<LogicalPath> for V1 { type Data = HList![SentRecords]; }
//! # impl Share<LinkPath> for V1 { type Data = HList![LinkCore, Planner, Label]; }
//! # impl Share<Recovery> for V1 { type Data = HList![Congestion]; }
//! # macro_rules! lends {
//! #     ($l:ty, $n:ty) => {
//! #         nitori_driver::__private::assert_lends::<Conn, HList![V1], $l, $n, _, _>()
//! #     };
//! # }
//! lends!(
//!     LinkPath,
//!     HList![
//!         Mut<Planner>,
//!         Mut<SentRecords>,
//!         Under<ConnCore, Ref<LinkCore>>,
//!         Under<LinkCore, Mut<Congestion>>,
//!         OwnKey<LinkId>,
//!     ]
//! );
//! ```
//!
//! Two needs that take the same component of one kind, one of them
//! exclusive, overlap: the sent records of the lend point's logical path
//! are among those of every logical path.
//!
//! ```compile_fail,E0277
//! # use nitori_driver::{
//! #     frunk_core::HList,
//! #     id::Numbered,
//! #     need::*,
//! #     structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
//! # };
//! # #[derive(Numbered)] struct ConnCore;
//! # #[derive(Numbered)] struct SentRecords;
//! # #[derive(Numbered)] struct LinkCore;
//! # #[derive(Numbered)] struct Planner;
//! # #[derive(Numbered)] struct Congestion;
//! # #[derive(Numbered)] struct Label;
//! # #[derive(Clone, PartialEq, Eq)] struct PathKey(u32);
//! # #[derive(Clone, PartialEq, Eq)] struct LinkId(u32);
//! # struct Conn;
//! # impl NodeKind for Conn { type Children = HList![Child<LogicalPath, SlotVec<PathKey>>]; }
//! # struct LogicalPath;
//! # impl NodeKind for LogicalPath { type Children = HList![Child<LinkPath, SlotVec<LinkId>>]; }
//! # struct LinkPath;
//! # impl NodeKind for LinkPath { type Children = HList![Child<Recovery, AtMostOne>]; }
//! # struct Recovery;
//! # impl NodeKind for Recovery { type Children = HList![]; }
//! # struct V1;
//! # impl Share<Conn> for V1 { type Data = HList![ConnCore, Label]; }
//! # impl Share<LogicalPath> for V1 { type Data = HList![SentRecords]; }
//! # impl Share<LinkPath> for V1 { type Data = HList![LinkCore, Planner, Label]; }
//! # impl Share<Recovery> for V1 { type Data = HList![Congestion]; }
//! # macro_rules! lends {
//! #     ($l:ty, $n:ty) => {
//! #         nitori_driver::__private::assert_lends::<Conn, HList![V1], $l, $n, _, _>()
//! #     };
//! # }
//! lends!(LinkPath, HList![Mut<SentRecords>, Under<ConnCore, Mut<SentRecords>>]);
//! ```
//!
//! Two shared needs of one component do not.
//!
//! ```
//! # use nitori_driver::{
//! #     frunk_core::HList,
//! #     id::Numbered,
//! #     need::*,
//! #     structure::{AtMostOne, Child, NodeKind, Share, SlotVec},
//! # };
//! # #[derive(Numbered)] struct ConnCore;
//! # #[derive(Numbered)] struct SentRecords;
//! # #[derive(Numbered)] struct LinkCore;
//! # #[derive(Numbered)] struct Planner;
//! # #[derive(Numbered)] struct Congestion;
//! # #[derive(Numbered)] struct Label;
//! # #[derive(Clone, PartialEq, Eq)] struct PathKey(u32);
//! # #[derive(Clone, PartialEq, Eq)] struct LinkId(u32);
//! # struct Conn;
//! # impl NodeKind for Conn { type Children = HList![Child<LogicalPath, SlotVec<PathKey>>]; }
//! # struct LogicalPath;
//! # impl NodeKind for LogicalPath { type Children = HList![Child<LinkPath, SlotVec<LinkId>>]; }
//! # struct LinkPath;
//! # impl NodeKind for LinkPath { type Children = HList![Child<Recovery, AtMostOne>]; }
//! # struct Recovery;
//! # impl NodeKind for Recovery { type Children = HList![]; }
//! # struct V1;
//! # impl Share<Conn> for V1 { type Data = HList![ConnCore, Label]; }
//! # impl Share<LogicalPath> for V1 { type Data = HList![SentRecords]; }
//! # impl Share<LinkPath> for V1 { type Data = HList![LinkCore, Planner, Label]; }
//! # impl Share<Recovery> for V1 { type Data = HList![Congestion]; }
//! # macro_rules! lends {
//! #     ($l:ty, $n:ty) => {
//! #         nitori_driver::__private::assert_lends::<Conn, HList![V1], $l, $n, _, _>()
//! #     };
//! # }
//! lends!(LinkPath, HList![Ref<SentRecords>, Under<ConnCore, Ref<SentRecords>>]);
//! ```
use core::marker::PhantomData;

mod one_at_a_time;
mod plus;
mod view;

pub use one_at_a_time::{OneAtATime, OneView};
pub use plus::{BranchAt, Branches, Plus, PlusView, Take};
pub use view::{
    EmptyPos, KeyedBy, KeyedView, Members, Need, Optional, Pos, Shape, Single, SingleView, View,
};

/// The component `X`, exclusive, of the one node from the lend point up to
/// the root that has it.
///
/// No node on the way up having `X`, or more than one, does not compile:
/// from a link path, no node above has the congestion, which is below it,
/// and two have the label.
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
/// lends!(LinkPath, HList![Mut<Congestion>]);
/// ```
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
/// lends!(LinkPath, HList![Mut<Label>]);
/// ```
pub struct Mut<X>(PhantomData<fn() -> X>);
impl<X: 'static> Need for Mut<X> {
    type View<'t, P: Pos> = &'t mut X;
}

/// The component `X`, shared, of the one node from the lend point up to
/// the root that has it.
///
/// No node on the way up having `X`, or more than one, does not compile.
pub struct Ref<X>(PhantomData<fn() -> X>);
impl<X: 'static> Need for Ref<X> {
    type View<'t, P: Pos> = &'t X;
}

/// `T` taken at every node of the one kind, at or below the anchor, that
/// has the component `T` takes; the anchor is the one node from the lend
/// point up to the root that has the component `A`.
///
/// The anchor is part of the range: a member may be the anchor itself. `T`
/// is [`Mut`]`<X>` or [`Ref`]`<X>`, which take `X` at each member, or
/// [`At`]`<B, T2>`, which makes each member, the node that has `B`, the
/// lend point of `T2`. The members must be borrowed apart: what `T` takes
/// exclusive is at the member or below it, never above, where the members
/// share it; [`OneAtATime`]`<T>` lifts this, handing out one member at a
/// time. How many members there are follows from the structure
/// ([`Shape`]).
///
/// No node on the way up having `A`, or more than one; no kind at or below
/// the anchor having the component, or more than one kind; or an exclusive
/// need above the members does not compile. Below the connection, two
/// kinds have the label:
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
/// lends!(LinkPath, HList![Under<ConnCore, Mut<Label>>]);
/// ```
///
/// and below the logical path, no kind has the connection's core:
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
/// lends!(LinkPath, HList![Under<SentRecords, Ref<ConnCore>>]);
/// ```
///
/// Every recovery's congestion can be taken exclusive with the sent
/// records of its logical path shared, but not exclusive, since the
/// recoveries of one logical path share them:
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
/// lends!(Conn, HList![Under<ConnCore, At<Congestion, HList![Mut<Congestion>, Ref<SentRecords>]>>]);
/// ```
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
/// lends!(Conn, HList![Under<ConnCore, At<Congestion, HList![Mut<Congestion>, Mut<SentRecords>]>>]);
/// ```
pub struct Under<A, T>(PhantomData<fn() -> (A, T)>);
impl<A: 'static, T: MemberNeed> Need for Under<A, T> {
    type View<'t, P: Pos> = T::Collected<'t, P>;
}

/// A need taken at each member of an [`Under`], and what the members'
/// views are gathered into.
pub trait MemberNeed: Need {
    /// The result of the `Under`, lent for `'t` at its position `P`.
    type Collected<'t, P: Pos>;
}
impl<X: 'static> MemberNeed for Mut<X> {
    type Collected<'t, P: Pos> = <P::Shape as Shape>::Out<Self::View<'t, P::Inner>>;
}
impl<X: 'static> MemberNeed for Ref<X> {
    type Collected<'t, P: Pos> = <P::Shape as Shape>::Out<Self::View<'t, P::Inner>>;
}
impl<B: 'static, T: Need> MemberNeed for At<B, T> {
    type Collected<'t, P: Pos> = <P::Shape as Shape>::Out<Self::View<'t, P::Inner>>;
}

/// `T` taken with the node that has the component `B` as the lend point:
/// the one node from the lend point up to the root that has it, or each
/// member inside an [`Under`].
///
/// `T` is any need, taken up and down from the new lend point.
pub struct At<B, T>(PhantomData<fn() -> (B, T)>);
impl<B: 'static, T: Need> Need for At<B, T> {
    type View<'t, P: Pos> = T::View<'t, P::Inner>;
}

/// The key the lend point is kept under by its parent, `K`, cloned.
///
/// The root has no key, and a lend point kept under another type of key
/// does not compile; a node kept by an at-most-one child has the key `()`.
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
/// lends!(LinkPath, HList![OwnKey<LinkId>, At<SentRecords, HList![OwnKey<PathKey>]>]);
/// lends!(Recovery, HList![OwnKey<()>]);
/// ```
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
/// lends!(Conn, HList![OwnKey<PathKey>]);
/// ```
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
/// lends!(LinkPath, HList![OwnKey<PathKey>]);
/// ```
pub struct OwnKey<K>(PhantomData<fn() -> K>);
impl<K: 'static> Need for OwnKey<K> {
    type View<'t, P: Pos> = K;
}

/// The address of the lend point by keys, from the root
/// ([`KeyAddress`](crate::structure::KeyAddress)).
pub struct OwnAddress;
impl Need for OwnAddress {
    type View<'t, P: Pos> = P::Address;
}
