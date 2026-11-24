//! The entries the driver calls components' operations and its own
//! reaction points through: each a lending, proved as a call's is.
//!
//! Every entry is generic over the positions of what it lends, inferred
//! where the structure is known, or given by a caller that proved them
//! elsewhere.
use core::marker::PhantomData;

use frunk_core::hlist::{HCons, HNil};

use super::{
    Lend, LendContext, PointCx, PointFk,
    build::{NewPointCx, NewPointFk, ParentRoute, RouteParent, lend_new},
    check::{Check, DeepEnoughAll, Disjoint, Takes},
    cycle::NoCycle,
    lend,
    list::Prefix,
    plan::Run,
    table::TabAt,
    this::This,
    tree::{Ctx, Locate},
};
use crate::{
    component::{ArgsOf, Component, EndCause, PartList},
    id::Numbered,
    need::{EmptyPos, Need, Pos, View},
    react::{AboveLendPoint, NodeEnding, NodeStarted, Reactions, WithLendPoint},
    structure::{KindFor, Node, NodeKind, Share},
};

// ------------------------------------------------------------------ lending

/// The needs `Self`, at the position `P`, lend at a node of the kind `L`
/// of the tree whose root is `R`, for the parts `Pt`, with the context
/// `X`: what [`lend`] proves, as one bound.
///
/// It is not a subtrait of [`Need`], so that a bound of it on a list of
/// needs leaves the views of the list to be worked out from the needs.
pub(crate) trait Lends<R: KindFor<Pt>, Pt, L, IL, X, P: Pos> {
    /// The lend point's full key.
    type Fk;
    /// Lend the needs at the node with the full key `at` to `f`.
    fn lend_to<F, O>(root: &mut Node<R, Pt>, at: &Self::Fk, cx: &mut X, f: F) -> Option<O>
    where
        Self: Need,
        F: for<'v> FnOnce(<Self as Need>::View<'v, P>) -> O;
}
impl<R, Pt: 'static, L, IL, N, P, X> Lends<R, Pt, L, IL, X, P> for N
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    PointCx<R, Pt, L, IL, X>: Ctx,
    N: NoCycle<R, Pt, HNil>,
    N: Check<PointCx<R, Pt, L, IL, X>, P>,
    N: Takes<PointCx<R, Pt, L, IL, X>, P>,
    <N as Takes<PointCx<R, Pt, L, IL, X>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<PointCx<R, Pt, L, IL, X>, P>,
    P: Pos,
    R: TabAt<Pt, HNil, <R as Locate<L, IL>>::Route, AtFk = PointFk<R, Pt, L, IL, X>>,
    X: LendContext,
    N::Cols: Run<R, Pt, X>,
{
    type Fk = PointFk<R, Pt, L, IL, X>;
    fn lend_to<F, O>(root: &mut Node<R, Pt>, at: &Self::Fk, cx: &mut X, f: F) -> Option<O>
    where
        N: Need,
        F: for<'v> FnOnce(N::View<'v, P>) -> O,
    {
        lend::<R, Pt, L, IL, N, P, X, F, O>(root, at, cx, f)
    }
}

/// The needs `Self`, at the position `P`, lend at a new node of the kind
/// `L`, not in the tree yet: what [`lend_new`] proves, as one bound.
pub(crate) trait LendsNew<R: KindFor<Pt>, Pt, L, IL, X, P: Pos> {
    /// The new node's full key.
    type Fk;
    /// Lend the needs at the new node with the full key `at` to `f`.
    fn lend_new<F, O>(root: &mut Node<R, Pt>, at: &Self::Fk, cx: &mut X, f: F) -> Option<O>
    where
        Self: Need,
        F: for<'v> FnOnce(<Self as Need>::View<'v, P>) -> O;
}
impl<R, Pt: 'static, L, IL, N, P, X> LendsNew<R, Pt, L, IL, X, P> for N
where
    R: KindFor<Pt> + NodeKind + Locate<L, IL>,
    NewPointCx<R, Pt, L, IL, X>: Ctx,
    N: NoCycle<R, Pt, HNil>,
    N: Check<NewPointCx<R, Pt, L, IL, X>, P>,
    N: Takes<NewPointCx<R, Pt, L, IL, X>, P>,
    <N as Takes<NewPointCx<R, Pt, L, IL, X>, P>>::Touches: Disjoint + DeepEnoughAll,
    N: Lend<NewPointCx<R, Pt, L, IL, X>, P>,
    P: Pos,
    <R as Locate<L, IL>>::Route: RouteParent,
    R: TabAt<Pt, HNil, ParentRoute<R, L, IL>>,
    NewPointFk<R, Pt, L, IL, X>: Prefix<<R as TabAt<Pt, HNil, ParentRoute<R, L, IL>>>::AtFk>,
    X: LendContext,
    N::Cols: Run<R, Pt, X>,
{
    type Fk = NewPointFk<R, Pt, L, IL, X>;
    fn lend_new<F, O>(root: &mut Node<R, Pt>, at: &Self::Fk, cx: &mut X, f: F) -> Option<O>
    where
        N: Need,
        F: for<'v> FnOnce(N::View<'v, P>) -> O,
    {
        lend_new::<R, Pt, L, IL, N, P, X, F, O>(root, at, cx, f)
    }
}

// ------------------------------------------------------------------ waking and ending

/// What a component's `wake` and `end` are lent: the component itself,
/// then its needs.
pub(crate) type OwnNeeds<C, O, L, K> = HCons<This<C>, <C as Component<O, L, K>>::Needs>;

/// Wake the component `C` of the part `O` at the node of the kind `K`
/// with the full key `at`; `None` when no node has that full key. `PT` is
/// the position of the component, `P` of its needs.
pub(crate) fn wake<R, Pt, K, IK, O, C, PT, P, X>(
    root: &mut Node<R, Pt>,
    at: &<OwnNeeds<C, O, Pt, K> as Lends<R, Pt, K, IK, X, HCons<PT, P>>>::Fk,
    cx: &mut X,
) -> Option<()>
where
    R: KindFor<Pt>,
    Pt: PartList<Pt>,
    K: NodeKind,
    C: Component<O, Pt, K>,
    PT: Pos,
    P: Pos,
    OwnNeeds<C, O, Pt, K>: Lends<R, Pt, K, IK, X, HCons<PT, P>>,
{
    <OwnNeeds<C, O, Pt, K> as Lends<R, Pt, K, IK, X, HCons<PT, P>>>::lend_to(root, at, cx, |view| {
        C::wake::<P>(view)
    })
}

/// End the component `C` of the part `O` at the node of the kind `K`
/// with the full key `at`, for `cause`; `None` when no node has that
/// full key.
pub(crate) fn end<R, Pt, K, IK, O, C, PT, P, X>(
    root: &mut Node<R, Pt>,
    at: &<OwnNeeds<C, O, Pt, K> as Lends<R, Pt, K, IK, X, HCons<PT, P>>>::Fk,
    cx: &mut X,
    cause: EndCause<'_, K::EndReason>,
) -> Option<()>
where
    R: KindFor<Pt>,
    Pt: PartList<Pt>,
    K: NodeKind,
    C: Component<O, Pt, K>,
    PT: Pos,
    P: Pos,
    OwnNeeds<C, O, Pt, K>: Lends<R, Pt, K, IK, X, HCons<PT, P>>,
{
    <OwnNeeds<C, O, Pt, K> as Lends<R, Pt, K, IK, X, HCons<PT, P>>>::lend_to(root, at, cx, |view| {
        C::end::<P>(view, cause)
    })
}

// ------------------------------------------------------------------ building

/// Building the components of a list `Self` of the part `O`, for a new
/// node of the kind `K` with the full key `Fk`, one lending each; `Ps`
/// are the positions of their build needs.
pub(crate) trait BuildComps<R: KindFor<Pt>, Pt: PartList<Pt>, K: NodeKind, IK, X, Fk, O, Ps>:
    Sized
{
    /// The components, in order.
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self>;
}
impl<R: KindFor<Pt>, Pt: PartList<Pt>, K: NodeKind, IK, X, Fk, O>
    BuildComps<R, Pt, K, IK, X, Fk, O, HNil> for HNil
{
    fn build(
        _: &mut Node<R, Pt>,
        _: &Fk,
        _: &mut K::Start,
        _: &ArgsOf<Pt>,
        _: &mut X,
    ) -> Option<HNil> {
        Some(HNil)
    }
}
impl<R, Pt, K, IK, X, Fk, O, C, Cs, PC, PT> BuildComps<R, Pt, K, IK, X, Fk, O, HCons<PC, PT>>
    for HCons<C, Cs>
where
    R: KindFor<Pt>,
    Pt: PartList<Pt>,
    K: NodeKind,
    C: Component<O, Pt, K>,
    PC: Pos,
    C::BuildNeeds: LendsNew<R, Pt, K, IK, X, PC, Fk = Fk>,
    Cs: BuildComps<R, Pt, K, IK, X, Fk, O, PT>,
{
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self> {
        let head =
            <C::BuildNeeds as LendsNew<R, Pt, K, IK, X, PC>>::lend_new(root, at, cx, |view| {
                C::build::<PC>(start, args, view)
            })?;
        let tail = Cs::build(root, at, start, args, cx)?;
        Some(HCons { head, tail })
    }
}

/// Building the shares of the parts `Self` for a new node of the kind
/// `K`, in the order of the parts.
pub(crate) trait BuildShares<R: KindFor<Pt>, Pt: PartList<Pt>, K: NodeKind, IK, X, Fk, Ps> {
    /// The shares, one list per part.
    type Data;
    /// Every part's share.
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self::Data>;
}
impl<R: KindFor<Pt>, Pt: PartList<Pt>, K: NodeKind, IK, X, Fk>
    BuildShares<R, Pt, K, IK, X, Fk, HNil> for HNil
{
    type Data = HNil;
    fn build(
        _: &mut Node<R, Pt>,
        _: &Fk,
        _: &mut K::Start,
        _: &ArgsOf<Pt>,
        _: &mut X,
    ) -> Option<HNil> {
        Some(HNil)
    }
}
impl<R, Pt, K, IK, X, Fk, O, Os, PO, PT> BuildShares<R, Pt, K, IK, X, Fk, HCons<PO, PT>>
    for HCons<O, Os>
where
    R: KindFor<Pt>,
    Pt: PartList<Pt>,
    K: NodeKind,
    O: Share<K>,
    O::Data: BuildComps<R, Pt, K, IK, X, Fk, O, PO>,
    Os: BuildShares<R, Pt, K, IK, X, Fk, PT>,
{
    type Data = HCons<O::Data, Os::Data>;
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self::Data> {
        let head = O::Data::build(root, at, start, args, cx)?;
        let tail = Os::build(root, at, start, args, cx)?;
        Some(HCons { head, tail })
    }
}

/// The shares of a new node of the kind `K` with the full key `at`, below
/// a node of the tree: every component built in the order of the parts,
/// each lent its build needs from the nodes above; `None` when the parent
/// is not in the tree. `Ps` are the positions of the build needs.
pub(crate) fn build_node<R, Pt, K, IK, X, Ps>(
    root: &mut Node<R, Pt>,
    at: &NewPointFk<R, Pt, K, IK, X>,
    start: &mut K::Start,
    args: &ArgsOf<Pt>,
    cx: &mut X,
) -> Option<<K as KindFor<Pt>>::Data>
where
    R: KindFor<Pt> + NodeKind + Locate<K, IK>,
    K: KindFor<Pt>,
    NewPointCx<R, Pt, K, IK, X>: Ctx,
    Pt: PartList<Pt>
        + BuildShares<
            R,
            Pt,
            K,
            IK,
            X,
            NewPointFk<R, Pt, K, IK, X>,
            Ps,
            Data = <K as KindFor<Pt>>::Data,
        >,
{
    <Pt as BuildShares<R, Pt, K, IK, X, NewPointFk<R, Pt, K, IK, X>, Ps>>::build(
        root, at, start, args, cx,
    )
}

/// The build needs of a component of the root: none, since nothing is
/// above the root.
#[diagnostic::on_unimplemented(
    message = "a component of the root has the build needs `{Self}`, and nothing is above the root to take them from",
    label = "not empty",
    note = "the components of the root build from the start and the arguments only: their `BuildNeeds` is `HNil`"
)]
pub(crate) trait RootBuildNeeds: Need {
    /// The view of no needs.
    fn none<'t, P: Pos>() -> View<'t, Self, P>;
}
impl RootBuildNeeds for HNil {
    fn none<'t, P: Pos>() -> View<'t, Self, P> {
        HNil
    }
}

/// Building the components of a list `Self` of the part `O` for the root
/// of the kind `R`.
pub(crate) trait BuildRootComps<R: NodeKind, Pt: PartList<Pt>, O>: Sized {
    /// The components, in order.
    fn build(start: &mut R::Start, args: &ArgsOf<Pt>) -> Self;
}
impl<R: NodeKind, Pt: PartList<Pt>, O> BuildRootComps<R, Pt, O> for HNil {
    fn build(_: &mut R::Start, _: &ArgsOf<Pt>) -> HNil {
        HNil
    }
}
impl<R: NodeKind, Pt: PartList<Pt>, O, C, Cs> BuildRootComps<R, Pt, O> for HCons<C, Cs>
where
    C: Component<O, Pt, R>,
    C::BuildNeeds: RootBuildNeeds,
    Cs: BuildRootComps<R, Pt, O>,
{
    fn build(start: &mut R::Start, args: &ArgsOf<Pt>) -> Self {
        let head = C::build::<EmptyPos>(
            start,
            args,
            <C::BuildNeeds as RootBuildNeeds>::none::<EmptyPos>(),
        );
        HCons {
            head,
            tail: Cs::build(start, args),
        }
    }
}
/// Building the shares of the parts `Self` for the root of the kind `R`.
pub(crate) trait BuildRootShares<R: NodeKind, Pt: PartList<Pt>> {
    /// The shares, one list per part.
    type Data;
    /// Every part's share.
    fn build(start: &mut R::Start, args: &ArgsOf<Pt>) -> Self::Data;
}
impl<R: NodeKind, Pt: PartList<Pt>> BuildRootShares<R, Pt> for HNil {
    type Data = HNil;
    fn build(_: &mut R::Start, _: &ArgsOf<Pt>) -> HNil {
        HNil
    }
}
impl<R: NodeKind, Pt: PartList<Pt>, O, Os> BuildRootShares<R, Pt> for HCons<O, Os>
where
    O: Share<R>,
    O::Data: BuildRootComps<R, Pt, O>,
    Os: BuildRootShares<R, Pt>,
{
    type Data = HCons<O::Data, Os::Data>;
    fn build(start: &mut R::Start, args: &ArgsOf<Pt>) -> Self::Data {
        let head = O::Data::build(start, args);
        HCons {
            head,
            tail: Os::build(start, args),
        }
    }
}

/// The shares of the root of the kind `R`: every component built in the
/// order of the parts, from `start` and the arguments.
pub(crate) fn build_root<R, Pt>(start: &mut R::Start, args: &ArgsOf<Pt>) -> <R as KindFor<Pt>>::Data
where
    R: KindFor<Pt>,
    Pt: PartList<Pt> + BuildRootShares<R, Pt, Data = <R as KindFor<Pt>>::Data>,
{
    <Pt as BuildRootShares<R, Pt>>::build(start, args)
}

// ------------------------------------------------------------------ the driver's points

/// What [`NodeStarted<K>`] takes: the reactions of `K` and of the kinds
/// above it.
pub(crate) type StartedReactions<K> = Reactions<NodeStarted<K>, WithLendPoint>;

/// Fire [`NodeStarted<K>`] at the node of the kind `K` with the full key
/// `at`, just added; `None` when no node has that full key.
pub(crate) fn node_started<R, Pt, K, IK, P, X>(
    root: &mut Node<R, Pt>,
    at: &<StartedReactions<K> as Lends<R, Pt, K, IK, X, P>>::Fk,
    cx: &mut X,
) -> Option<()>
where
    R: KindFor<Pt>,
    K: NodeKind,
    NodeStarted<K>: Numbered,
    P: Pos,
    StartedReactions<K>: Lends<R, Pt, K, IK, X, P>,
{
    <StartedReactions<K> as Lends<R, Pt, K, IK, X, P>>::lend_to(root, at, cx, |mut reactions| {
        reactions.fire(())
    })
}

/// What [`NodeEnding<K>`] takes: the reactions of the kinds above `K`.
pub(crate) type EndingReactions<K> = Reactions<NodeEnding<K>, AboveLendPoint>;

/// Fire [`NodeEnding<K>`] at the node of the kind `K` with the full key
/// `at`, still in the tree; `None` when no node has that full key.
pub(crate) fn node_ending<R, Pt, K, IK, P, X>(
    root: &mut Node<R, Pt>,
    at: &<EndingReactions<K> as Lends<R, Pt, K, IK, X, P>>::Fk,
    cx: &mut X,
) -> Option<()>
where
    R: KindFor<Pt>,
    K: NodeKind,
    NodeEnding<K>: Numbered,
    P: Pos,
    EndingReactions<K>: Lends<R, Pt, K, IK, X, P>,
{
    <EndingReactions<K> as Lends<R, Pt, K, IK, X, P>>::lend_to(root, at, cx, |mut reactions| {
        reactions.fire(())
    })
}

/// Prove what a component `C` at the kind `L` of the tree whose root is
/// `R`, for the parts `Pt`, with the needs `N`, is lent: itself, then
/// `N`, for the examples of the documentation.
#[doc(hidden)]
#[allow(private_bounds)]
pub fn assert_component_lends<R, Pt, L, C, N, IL, P>()
where
    R: KindFor<Pt>,
    P: Pos,
    HCons<This<C>, N>: Lends<R, Pt, L, IL, (), P>,
{
    let _ = PhantomData::<fn() -> (R, Pt, L, C, N, IL, P)>;
}
