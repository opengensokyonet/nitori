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
    table::{KidRoute, KindTab, TabAt},
    this::This,
    tree::{Ctx, InKid, IsSelf, Locate},
};
use crate::{
    call::{
        __pack::{Desc, KindDesc},
        Composition, ContextOf, PartsOf, RootOf,
    },
    component::{ArgsOf, Component, EndCause, Part, PartList},
    id::Numbered,
    need::{EmptyPos, Need, Pos, View},
    react::{AboveLendPoint, NodeEnding, NodeStarted, Reactions, WithLendPoint},
    structure::{KindFor, Node, NodeKind, ShareAt},
};

// ------------------------------------------------------------------ lending

/// The needs `Self`, at the position `P`, lend at a node of the kind `L`
/// of the tree whose root is `R`, for the parts `Pt`, with the context
/// `X`: what [`lend`] proves, as one bound.
///
/// It is not a subtrait of [`Need`], so that a bound of it on a list of
/// needs leaves the views of the list to be worked out from the needs.
#[diagnostic::on_unimplemented(
    note = "to see why, prove the one lending on its own with `nitori_driver::explain!`: `explain!(Q, Kind, call C)`, `explain!(Q, Kind, component C)` or `explain!(Q, Kind, needs![...])`, whose error says which need fails and why"
)]
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
    R: KindTab<Pt, HNil>,
    R::Children: KidRoute<Pt, HNil, <R as Locate<L, IL>>::Route, PointFk<R, Pt, L, IL, X>>,
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
    R::Children: KidRoute<
            Pt,
            HNil,
            ParentRoute<R, L, IL>,
            <R as TabAt<Pt, HNil, ParentRoute<R, L, IL>>>::AtFk,
        >,
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
    O: Part<Pt>,
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
    O: Part<Pt>,
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
where
    O: Part<Pt>,
{
    /// The components, in order, each handed `own`, the arguments of
    /// `O`, and `args`, those of every part.
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        own: &O::Args,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self>;
}
impl<R: KindFor<Pt>, Pt: PartList<Pt>, K: NodeKind, IK, X, Fk, O: Part<Pt>>
    BuildComps<R, Pt, K, IK, X, Fk, O, HNil> for HNil
{
    fn build(
        _: &mut Node<R, Pt>,
        _: &Fk,
        _: &mut K::Start,
        _: &O::Args,
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
    O: Part<Pt>,
    C: Component<O, Pt, K>,
    PC: Pos,
    C::BuildNeeds: LendsNew<R, Pt, K, IK, X, PC, Fk = Fk>,
    Cs: BuildComps<R, Pt, K, IK, X, Fk, O, PT>,
{
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        own: &O::Args,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self> {
        let head =
            <C::BuildNeeds as LendsNew<R, Pt, K, IK, X, PC>>::lend_new(root, at, cx, |view| {
                C::build::<PC>(start, own, args, view)
            })?;
        let tail = Cs::build(root, at, start, own, args, cx)?;
        Some(HCons { head, tail })
    }
}

/// Building the shares of the parts `Self` for a new node of the kind
/// `K`, in the order of the parts.
pub(crate) trait BuildShares<R: KindFor<Pt>, Pt: PartList<Pt>, K: NodeKind, IK, X, Fk, Ps>:
    PartList<Pt>
{
    /// The shares, one list per part.
    type Data;
    /// Every part's share; `own` are the arguments of the parts `Self`,
    /// `args` those of every part.
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        own: &Self::Args,
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
        _: &HNil,
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
    O: Part<Pt> + ShareAt<K, Pt>,
    O::Data: BuildComps<R, Pt, K, IK, X, Fk, O, PO>,
    Os: BuildShares<R, Pt, K, IK, X, Fk, PT>,
{
    type Data = HCons<O::Data, Os::Data>;
    fn build(
        root: &mut Node<R, Pt>,
        at: &Fk,
        start: &mut K::Start,
        own: &Self::Args,
        args: &ArgsOf<Pt>,
        cx: &mut X,
    ) -> Option<Self::Data> {
        let head = O::Data::build(root, at, start, &own.head, args, cx)?;
        let tail = Os::build(root, at, start, &own.tail, args, cx)?;
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
        root, at, start, args, args, cx,
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
pub(crate) trait BuildRootComps<R: NodeKind, Pt: PartList<Pt>, O: Part<Pt>>: Sized {
    /// The components, in order, each handed `own`, the arguments of
    /// `O`, and `args`, those of every part.
    fn build(start: &mut R::Start, own: &O::Args, args: &ArgsOf<Pt>) -> Self;
}
impl<R: NodeKind, Pt: PartList<Pt>, O: Part<Pt>> BuildRootComps<R, Pt, O> for HNil {
    fn build(_: &mut R::Start, _: &O::Args, _: &ArgsOf<Pt>) -> HNil {
        HNil
    }
}
impl<R: NodeKind, Pt: PartList<Pt>, O: Part<Pt>, C, Cs> BuildRootComps<R, Pt, O> for HCons<C, Cs>
where
    C: Component<O, Pt, R>,
    C::BuildNeeds: RootBuildNeeds,
    Cs: BuildRootComps<R, Pt, O>,
{
    fn build(start: &mut R::Start, own: &O::Args, args: &ArgsOf<Pt>) -> Self {
        let head = C::build::<EmptyPos>(
            start,
            own,
            args,
            <C::BuildNeeds as RootBuildNeeds>::none::<EmptyPos>(),
        );
        HCons {
            head,
            tail: Cs::build(start, own, args),
        }
    }
}
/// Building the shares of the parts `Self` for the root of the kind `R`.
pub(crate) trait BuildRootShares<R: NodeKind, Pt: PartList<Pt>>: PartList<Pt> {
    /// The shares, one list per part.
    type Data;
    /// Every part's share; `own` are the arguments of the parts `Self`,
    /// `args` those of every part.
    fn build(start: &mut R::Start, own: &Self::Args, args: &ArgsOf<Pt>) -> Self::Data;
}
impl<R: NodeKind, Pt: PartList<Pt>> BuildRootShares<R, Pt> for HNil {
    type Data = HNil;
    fn build(_: &mut R::Start, _: &HNil, _: &ArgsOf<Pt>) -> HNil {
        HNil
    }
}
impl<R: NodeKind, Pt: PartList<Pt>, O, Os> BuildRootShares<R, Pt> for HCons<O, Os>
where
    O: Part<Pt> + ShareAt<R, Pt>,
    O::Data: BuildRootComps<R, Pt, O>,
    Os: BuildRootShares<R, Pt>,
{
    type Data = HCons<O::Data, Os::Data>;
    fn build(start: &mut R::Start, own: &Self::Args, args: &ArgsOf<Pt>) -> Self::Data {
        let head = O::Data::build(start, &own.head, args);
        HCons {
            head,
            tail: Os::build(start, &own.tail, args),
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
    <Pt as BuildRootShares<R, Pt>>::build(start, args, args)
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

/// Where the build needs `N` of a component of a new node of the kind
/// `L` are lent, the way down to it from the root `R` being `Self`, for
/// the parts `Pt`, with the context `X`: below the root, `N` at the new
/// node, from the nodes above it, as [`LendsNew`] lends them; at the root,
/// nothing, `N` having to be none.
pub(crate) trait BuildSite<R, Pt, L, X, N> {
    /// What is lent from the nodes above.
    type Needs;
    /// The context they are proved in.
    type Cx: Ctx;
    /// What has to be none, since nothing is above the root.
    type AtRoot;
}
impl<R, Pt: 'static, L, X, N> BuildSite<R, Pt, L, X, N> for IsSelf
where
    R: KindFor<Pt> + NodeKind + Locate<L, IsSelf>,
    PointCx<R, Pt, L, IsSelf, X>: Ctx,
{
    type Needs = HNil;
    type Cx = PointCx<R, Pt, L, IsSelf, X>;
    type AtRoot = N;
}
impl<R, Pt: 'static, L, X, N, I> BuildSite<R, Pt, L, X, N> for InKid<I>
where
    R: KindFor<Pt> + NodeKind + Locate<L, InKid<I>>,
    NewPointCx<R, Pt, L, InKid<I>, X>: Ctx,
    <R as Locate<L, InKid<I>>>::Route: RouteParent,
    R: TabAt<Pt, HNil, ParentRoute<R, L, InKid<I>>>,
    NewPointFk<R, Pt, L, InKid<I>, X>:
        Prefix<<R as TabAt<Pt, HNil, ParentRoute<R, L, InKid<I>>>>::AtFk>,
    R::Children: KidRoute<
            Pt,
            HNil,
            ParentRoute<R, L, InKid<I>>,
            <R as TabAt<Pt, HNil, ParentRoute<R, L, InKid<I>>>>::AtFk,
        >,
{
    type Needs = N;
    type Cx = NewPointCx<R, Pt, L, InKid<I>, X>;
    type AtRoot = HNil;
}
/// The way down to the kind `K` of the composition `Q`.
type IlAt<Q, K> = <Desc<K> as KindDesc<Q>>::Il;
/// What the build needs `N` at the kind `K` of `Q` are lent as.
type SiteOf<Q, K, N> = <IlAt<Q, K> as BuildSite<RootOf<Q>, PartsOf<Q>, K, ContextOf<Q>, N>>::Needs;
/// The context they are proved in.
type SiteCx<Q, K, N> = <IlAt<Q, K> as BuildSite<RootOf<Q>, PartsOf<Q>, K, ContextOf<Q>, N>>::Cx;

/// Prove the build needs `N` of a component at the kind `K` of the
/// composition `Q` as where `Q` is composed: lent at a new node from the
/// nodes above it ([`LendsNew`]), and none at the root, with the checks
/// written out so that an error names the need. What
/// [`explain!`](crate::explain!) proves a component's build with.
#[doc(hidden)]
#[allow(private_bounds)]
pub fn assert_build_lends<Q, K, N, P>()
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    IlAt<Q, K>: BuildSite<RootOf<Q>, PartsOf<Q>, K, ContextOf<Q>, N>,
    <IlAt<Q, K> as BuildSite<RootOf<Q>, PartsOf<Q>, K, ContextOf<Q>, N>>::AtRoot: RootBuildNeeds,
    SiteOf<Q, K, N>: NoCycle<RootOf<Q>, PartsOf<Q>, HNil>,
    SiteOf<Q, K, N>: Check<SiteCx<Q, K, N>, P>,
    SiteOf<Q, K, N>: Takes<SiteCx<Q, K, N>, P>,
    <SiteOf<Q, K, N> as Takes<SiteCx<Q, K, N>, P>>::Touches: Disjoint + DeepEnoughAll,
    SiteOf<Q, K, N>: Lend<SiteCx<Q, K, N>, P>,
    P: Pos,
    ContextOf<Q>: LendContext,
{
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
