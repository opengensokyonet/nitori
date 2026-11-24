//! The pack of a composition: for every kind of its structure, what the
//! composition proved of it, reached through what is declared here; and
//! the entries the driver lends through it.
//!
//! The pack is a list with one item for each kind of the structure, in the
//! order of [`KindsOf`]: the positions of every call ([`CallList`]) and
//! every component ([`CompList`]) of the kind, of its build, and of the
//! reaction points the driver fires at it ([`KindOps`]). The composition
//! defines it as an opaque type, so code generic over the composition uses
//! only these traits; an empty list ends in a filler ([`NoKind`]) that is
//! never reached.
use core::{
    convert::Infallible,
    marker::PhantomData,
    pin::Pin,
    task::{self, Poll},
};

use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here, There},
};

use super::{
    Composition, ContextOf, KindsOf, PartsOf, RootNode, RootOf,
    kinds::{FindKind, ListRest, PartsCalls, PartsComps},
};
use crate::{
    component::{ArgsOf, EndCause},
    id::Numbered,
    lend::{
        PointCx, PointFk,
        build::{NewPointCx, NewPointFk},
        tree::{Ctx, InKid, IsSelf, Locate},
    },
    structure::{KindFor, NoChild, NodeKind},
};

// ------------------------------------------------------------------ kinds

/// A kind of the structure of `Q` as the pack reads it.
pub trait KindDesc<Q: Composition> {
    /// The kind ([`NoChild`] for the filler).
    type Kind: NodeKind;
    /// The way down to it from the root.
    type Il;
    /// The full key of its nodes.
    type Fk: Clone + PartialEq + 'static;
    /// What a new node of it is built at: its full key, or `()` for the
    /// root.
    type NewAt;
    /// The parts' shares in a node of it.
    type Data;
    /// The kinds of its calls, in the order they start.
    type Calls: ListRest;
    /// Its components, each a [`Comp`](super::Comp), in the order of the
    /// parts.
    type Comps: ListRest;
}
/// The kind `K` as an item of the pack's list.
pub struct Desc<K>(PhantomData<fn() -> K>);
/// The filler after the last kind of the pack's list.
pub enum NoKind {}

/// What a new node of the kind `K` is built at, `Self` being the way
/// down to it: `()` for the root, its full key otherwise.
pub trait NewAt<Q: Composition, K> {
    /// What it is built at.
    type Out;
}
impl<Q: Composition, K> NewAt<Q, K> for IsSelf {
    type Out = ();
}
impl<Q: Composition, K, I> NewAt<Q, K> for InKid<I>
where
    RootOf<Q>: Locate<K, InKid<I>>,
    NewPointCx<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>: Ctx,
{
    type Out = NewPointFk<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>;
}

type FoundIl<Q, K> = <RootOf<Q> as FindKind<K>>::Il;
impl<Q: Composition, K> KindDesc<Q> for Desc<K>
where
    K: KindFor<PartsOf<Q>> + Numbered,
    RootOf<Q>: FindKind<K> + Locate<K, FoundIl<Q, K>>,
    PointCx<RootOf<Q>, PartsOf<Q>, K, FoundIl<Q, K>, ContextOf<Q>>: Ctx,
    FoundIl<Q, K>: NewAt<Q, K>,
    PartsOf<Q>: PartsCalls<K> + PartsComps<K>,
{
    type Kind = K;
    type Il = FoundIl<Q, K>;
    type Fk = PointFk<RootOf<Q>, PartsOf<Q>, K, FoundIl<Q, K>, ContextOf<Q>>;
    type NewAt = <FoundIl<Q, K> as NewAt<Q, K>>::Out;
    type Data = <K as KindFor<PartsOf<Q>>>::Data;
    type Calls = <PartsOf<Q> as PartsCalls<K>>::Calls;
    type Comps = <PartsOf<Q> as PartsComps<K>>::Comps;
}
impl<Q: Composition> KindDesc<Q> for NoKind {
    type Kind = NoChild;
    type Il = ();
    type Fk = Infallible;
    type NewAt = Infallible;
    type Data = ();
    type Calls = HNil;
    type Comps = HNil;
}

/// The way down to the kind `K` of `Q`'s structure.
pub(crate) type IlOf<Q, K> = <Desc<K> as KindDesc<Q>>::Il;
/// The full key of the nodes of the kind `K` of `Q`'s tree.
pub(crate) type FkOf<Q, K> = <Desc<K> as KindDesc<Q>>::Fk;
/// What the kind `D` of the pack starts from.
pub(crate) type StartOf<Q, D> = <<D as KindDesc<Q>>::Kind as NodeKind>::Start;
/// Why the kind `D` of the pack ends.
pub(crate) type EndReasonOf<Q, D> = <<D as KindDesc<Q>>::Kind as NodeKind>::EndReason;

/// A list of kinds of `Q`'s structure as head and tail (the empty list:
/// [`NoKind`] and itself).
pub trait KindList<Q: Composition> {
    /// The first kind.
    type Head: KindDesc<Q>;
    /// The others.
    type Tail: KindList<Q>;
}
impl<Q: Composition> KindList<Q> for HNil {
    type Head = NoKind;
    type Tail = HNil;
}
impl<Q: Composition, K: Numbered, T: KindList<Q>> KindList<Q> for HCons<K, T>
where
    Desc<K>: KindDesc<Q>,
{
    type Head = Desc<K>;
    type Tail = T;
}

// ------------------------------------------------------------------ the pack

/// The pack of the kinds `L` of `Q`: one item per kind.
pub trait PackList<Q: Composition, L: KindList<Q>> {
    /// What was proved of the first kind.
    type Head: KindOps<Q, L::Head>;
    /// The others.
    type Tail: PackList<Q, L::Tail>;
}
impl<Q: Composition> PackList<Q, HNil> for HNil {
    type Head = NoKindPos;
    type Tail = HNil;
}
impl<Q: Composition, K: Numbered, T: KindList<Q>, P, PT> PackList<Q, HCons<K, T>> for HCons<P, PT>
where
    Desc<K>: KindDesc<Q>,
    P: KindOps<Q, Desc<K>>,
    PT: PackList<Q, T>,
{
    type Head = P;
    type Tail = PT;
}

/// What the composition proved of the kind `D` of `Q`: its calls, its
/// components, its build and the reaction points the driver fires at it.
pub trait KindOps<Q: Composition, D: KindDesc<Q>> {
    /// Its calls.
    type Calls: CallList<Q, D, D::Calls>;
    /// Its components.
    type Comps: CompList<Q, D, D::Comps>;
    /// The shares of a new node at `at`, every component built in the
    /// order of the parts from `start`, the arguments and its build needs;
    /// `None` when its parent is not in the tree.
    fn build(
        root: &mut RootNode<Q>,
        at: &D::NewAt,
        start: &mut StartOf<Q, D>,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<D::Data>;
    /// Fire [`NodeStarted`](crate::react::NodeStarted) at the node at
    /// `at`; `None` when it is not in the tree.
    fn started(root: &mut RootNode<Q>, at: &D::Fk, cx: &mut ContextOf<Q>) -> Option<()>;
    /// Fire [`NodeEnding`](crate::react::NodeEnding) at the node at
    /// `at`; `None` when it is not in the tree.
    fn ending(root: &mut RootNode<Q>, at: &D::Fk, cx: &mut ContextOf<Q>) -> Option<()>;
}
/// The filler's item of the pack.
pub struct NoKindPos;
impl<Q: Composition> KindOps<Q, NoKind> for NoKindPos {
    type Calls = HNil;
    type Comps = HNil;
    fn build(
        _: &mut RootNode<Q>,
        at: &Infallible,
        _: &mut Infallible,
        _: &ArgsOf<PartsOf<Q>>,
        _: &mut ContextOf<Q>,
    ) -> Option<()> {
        match *at {}
    }
    fn started(_: &mut RootNode<Q>, at: &Infallible, _: &mut ContextOf<Q>) -> Option<()> {
        match *at {}
    }
    fn ending(_: &mut RootNode<Q>, at: &Infallible, _: &mut ContextOf<Q>) -> Option<()> {
        match *at {}
    }
}

/// What the composition proved of the calls `Cs` of the kind `D`: the
/// first one's, and the others'.
pub trait CallList<Q: Composition, D: KindDesc<Q>, Cs: ListRest> {
    /// The first call as it runs between polls (`()` for none).
    type HeadRunning;
    /// The others.
    type Tail: CallList<Q, D, Cs::Rest>;
    /// The first call of a new node, from what is left of its start.
    fn start_head(start: &mut StartOf<Q, D>) -> Self::HeadRunning;
    /// Lend the first call its needs at the node at `at` and poll it once
    /// with `wcx`: `Ready` when it returned. `None` when the node is not
    /// in the tree.
    fn poll_head(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        call: Pin<&mut Self::HeadRunning>,
        wcx: &mut task::Context<'_>,
    ) -> Option<Poll<()>>;
}
impl<Q: Composition, D: KindDesc<Q>> CallList<Q, D, HNil> for HNil {
    type HeadRunning = ();
    type Tail = HNil;
    fn start_head(_: &mut StartOf<Q, D>) {}
    fn poll_head(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: Pin<&mut ()>,
        _: &mut task::Context<'_>,
    ) -> Option<Poll<()>> {
        Some(Poll::Ready(()))
    }
}

/// What the composition proved of the components `Cs` of the kind `D`:
/// the first one's, and the others'.
pub trait CompList<Q: Composition, D: KindDesc<Q>, Cs: ListRest> {
    /// The others.
    type Tail: CompList<Q, D, Cs::Rest>;
    /// Wake the first component at the node at `at`; `None` when it is
    /// not in the tree.
    fn wake_head(root: &mut RootNode<Q>, at: &D::Fk, cx: &mut ContextOf<Q>) -> Option<()>;
    /// End the first component at the node at `at` for `cause`; `None`
    /// when it is not in the tree.
    fn end_head(
        root: &mut RootNode<Q>,
        at: &D::Fk,
        cx: &mut ContextOf<Q>,
        cause: EndCause<'_, EndReasonOf<Q, D>>,
    ) -> Option<()>;
}
impl<Q: Composition, D: KindDesc<Q>> CompList<Q, D, HNil> for HNil {
    type Tail = HNil;
    fn wake_head(_: &mut RootNode<Q>, _: &D::Fk, _: &mut ContextOf<Q>) -> Option<()> {
        Some(())
    }
    fn end_head(
        _: &mut RootNode<Q>,
        _: &D::Fk,
        _: &mut ContextOf<Q>,
        _: EndCause<'_, EndReasonOf<Q, D>>,
    ) -> Option<()> {
        Some(())
    }
}

// ------------------------------------------------------------------ walking the pack

/// The item of the pack `Self` at the position `I` of the kinds `L`.
pub(crate) trait PackNth<Q: Composition, L: KindList<Q>, I> {
    /// The kinds from that position on.
    type List: KindList<Q>;
    /// The item.
    type Out: KindOps<Q, <Self::List as KindList<Q>>::Head>;
}
impl<Q: Composition, L: KindList<Q>, P: PackList<Q, L>> PackNth<Q, L, Here> for P {
    type List = L;
    type Out = P::Head;
}
impl<Q: Composition, L: KindList<Q>, P: PackList<Q, L>, I> PackNth<Q, L, There<I>> for P
where
    P::Tail: PackNth<Q, L::Tail, I>,
{
    type List = <P::Tail as PackNth<Q, L::Tail, I>>::List;
    type Out = <P::Tail as PackNth<Q, L::Tail, I>>::Out;
}

/// The calls `Self` from the position `J` of the calls `Cs` on.
pub(crate) trait NthCall<Q: Composition, D: KindDesc<Q>, Cs: ListRest, J> {
    /// The kinds of the calls from that position on.
    type Cs: ListRest;
    /// What was proved of them.
    type Pos: CallList<Q, D, Self::Cs>;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CallList<Q, D, Cs>> NthCall<Q, D, Cs, Here>
    for P
{
    type Cs = Cs;
    type Pos = P;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CallList<Q, D, Cs>, J>
    NthCall<Q, D, Cs, There<J>> for P
where
    P::Tail: NthCall<Q, D, Cs::Rest, J>,
{
    type Cs = <P::Tail as NthCall<Q, D, Cs::Rest, J>>::Cs;
    type Pos = <P::Tail as NthCall<Q, D, Cs::Rest, J>>::Pos;
}

/// The components `Self` from the position `J` of the components `Cs`
/// on.
pub(crate) trait NthComp<Q: Composition, D: KindDesc<Q>, Cs: ListRest, J> {
    /// The components from that position on.
    type Cs: ListRest;
    /// What was proved of them.
    type Pos: CompList<Q, D, Self::Cs>;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CompList<Q, D, Cs>> NthComp<Q, D, Cs, Here>
    for P
{
    type Cs = Cs;
    type Pos = P;
}
impl<Q: Composition, D: KindDesc<Q>, Cs: ListRest, P: CompList<Q, D, Cs>, J>
    NthComp<Q, D, Cs, There<J>> for P
where
    P::Tail: NthComp<Q, D, Cs::Rest, J>,
{
    type Cs = <P::Tail as NthComp<Q, D, Cs::Rest, J>>::Cs;
    type Pos = <P::Tail as NthComp<Q, D, Cs::Rest, J>>::Pos;
}

/// Every call `Cs` of a new node of the kind `D` started, in order.
pub(crate) trait StartCalls<Q: Composition, D: KindDesc<Q>, Cs: ListRest>:
    CallList<Q, D, Cs>
{
    /// The calls as they run, an `HList`.
    type Running;
    /// Start them all from what is left of the start, in order.
    fn start_all(start: &mut StartOf<Q, D>) -> Self::Running;
}
impl<Q: Composition, D: KindDesc<Q>, P: CallList<Q, D, HNil>> StartCalls<Q, D, HNil> for P {
    type Running = HNil;
    fn start_all(_: &mut StartOf<Q, D>) -> HNil {
        HNil
    }
}
impl<Q: Composition, D: KindDesc<Q>, C, Ct: ListRest, P> StartCalls<Q, D, HCons<C, Ct>> for P
where
    P: CallList<Q, D, HCons<C, Ct>>,
    P::Tail: StartCalls<Q, D, Ct>,
{
    type Running = HCons<P::HeadRunning, <P::Tail as StartCalls<Q, D, Ct>>::Running>;
    fn start_all(start: &mut StartOf<Q, D>) -> Self::Running {
        let head = P::start_head(start);
        HCons {
            head,
            tail: <P::Tail as StartCalls<Q, D, Ct>>::start_all(start),
        }
    }
}

// ------------------------------------------------------------------ the entries

/// What `Q` proved of the kind at the position `I` of its structure.
pub(crate) type KindPosAt<Q, I> = <<Q as Composition>::Pack as PackNth<Q, KindsOf<Q>, I>>::Out;
/// The kind at the position `I` of `Q`'s structure.
pub(crate) type DescAt<Q, I> =
    <<<Q as Composition>::Pack as PackNth<Q, KindsOf<Q>, I>>::List as KindList<Q>>::Head;
/// The full key of the nodes of the kind at `I`.
pub(crate) type FkAt<Q, I> = <DescAt<Q, I> as KindDesc<Q>>::Fk;
/// What the kind at `I` starts from.
pub(crate) type StartAt<Q, I> = StartOf<Q, DescAt<Q, I>>;
/// The kinds of the calls of the kind at `I`.
pub(crate) type CallKindsAt<Q, I> = <DescAt<Q, I> as KindDesc<Q>>::Calls;
/// The components of the kind at `I`.
pub(crate) type CompKindsAt<Q, I> = <DescAt<Q, I> as KindDesc<Q>>::Comps;
/// What `Q` proved of the calls of the kind at `I`.
pub(crate) type CallsAt<Q, I> = <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::Calls;
/// What `Q` proved of the components of the kind at `I`.
pub(crate) type CompsAt<Q, I> = <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::Comps;
/// The calls of a new node of the kind at `I` as they run, an `HList`.
pub(crate) type RunningCalls<Q, I> =
    <CallsAt<Q, I> as StartCalls<Q, DescAt<Q, I>, CallKindsAt<Q, I>>>::Running;
/// What `Q` proved of the calls of the kind at `I` from the position `J`
/// on.
pub(crate) type CallPosAt<Q, I, J> =
    <CallsAt<Q, I> as NthCall<Q, DescAt<Q, I>, CallKindsAt<Q, I>, J>>::Pos;
/// The kinds of the calls of the kind at `I` from the position `J` on.
pub(crate) type CallKindsFrom<Q, I, J> =
    <CallsAt<Q, I> as NthCall<Q, DescAt<Q, I>, CallKindsAt<Q, I>, J>>::Cs;
/// The call at the position `J` of the kind at `I` as it runs.
pub(crate) type RunningAt<Q, I, J> =
    <CallPosAt<Q, I, J> as CallList<Q, DescAt<Q, I>, CallKindsFrom<Q, I, J>>>::HeadRunning;
type CompPosAt<Q, I, J> = <CompsAt<Q, I> as NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>>::Pos;
type CompKindsFrom<Q, I, J> = <CompsAt<Q, I> as NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>>::Cs;

/// Start every call of a new node of the kind `K` (at `I`, inferred),
/// in the order of the parts, then of each part's calls, from what its
/// components left of its start.
pub(crate) fn start_calls<Q, K, I>(start: &mut StartAt<Q, I>) -> RunningCalls<Q, I>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CallsAt<Q, I>: StartCalls<Q, DescAt<Q, I>, CallKindsAt<Q, I>>,
{
    <CallsAt<Q, I> as StartCalls<Q, DescAt<Q, I>, CallKindsAt<Q, I>>>::start_all(start)
}

/// Lend the call of the kind `C` (at `J`, inferred) its needs at the node
/// of the kind `K` (at `I`, inferred) at `at`, and poll it once with
/// `wcx`: `Ready` when it returned, and the driver drops it. `None` when
/// the node is not in the tree.
pub(crate) fn poll_call<Q, K, C, I, J>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
    call: Pin<&mut RunningAt<Q, I, J>>,
    wcx: &mut task::Context<'_>,
) -> Option<Poll<()>>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CallKindsAt<Q, I>: Selector<C, J>,
    CallsAt<Q, I>: NthCall<Q, DescAt<Q, I>, CallKindsAt<Q, I>, J>,
{
    <CallPosAt<Q, I, J> as CallList<Q, DescAt<Q, I>, CallKindsFrom<Q, I, J>>>::poll_head(
        root, at, cx, call, wcx,
    )
}

/// Wake the component `C` of the part `O` (at `J`, inferred) at the node
/// of the kind `K` (at `I`, inferred) at `at`; `None` when the node is
/// not in the tree.
pub(crate) fn wake<Q, K, O, C, I, J>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CompKindsAt<Q, I>: Selector<super::Comp<O, C>, J>,
    CompsAt<Q, I>: NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>,
{
    <CompPosAt<Q, I, J> as CompList<Q, DescAt<Q, I>, CompKindsFrom<Q, I, J>>>::wake_head(
        root, at, cx,
    )
}

/// End the component `C` of the part `O` (at `J`, inferred) at the node
/// of the kind `K` (at `I`, inferred) at `at`, for `cause`; `None` when
/// the node is not in the tree.
pub(crate) fn end<Q, K, O, C, I, J>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
    cause: EndCause<'_, EndReasonOf<Q, DescAt<Q, I>>>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
    CompKindsAt<Q, I>: Selector<super::Comp<O, C>, J>,
    CompsAt<Q, I>: NthComp<Q, DescAt<Q, I>, CompKindsAt<Q, I>, J>,
{
    <CompPosAt<Q, I, J> as CompList<Q, DescAt<Q, I>, CompKindsFrom<Q, I, J>>>::end_head(
        root, at, cx, cause,
    )
}

/// The shares of a new node of the kind `K` (at `I`, inferred) at `at`:
/// every component built in the order of the parts; `None` when its
/// parent is not in the tree.
pub(crate) fn build<Q, K, I>(
    root: &mut RootNode<Q>,
    at: &<DescAt<Q, I> as KindDesc<Q>>::NewAt,
    start: &mut StartAt<Q, I>,
    args: &ArgsOf<PartsOf<Q>>,
    cx: &mut ContextOf<Q>,
) -> Option<<DescAt<Q, I> as KindDesc<Q>>::Data>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
{
    <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::build(root, at, start, args, cx)
}

/// Fire [`NodeStarted`](crate::react::NodeStarted) at the node of the kind
/// `K` (at `I`, inferred) at `at`; `None` when it is not in the tree.
pub(crate) fn node_started<Q, K, I>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
{
    <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::started(root, at, cx)
}

/// Fire [`NodeEnding`](crate::react::NodeEnding) at the node of the kind
/// `K` (at `I`, inferred) at `at`; `None` when it is not in the tree.
pub(crate) fn node_ending<Q, K, I>(
    root: &mut RootNode<Q>,
    at: &FkAt<Q, I>,
    cx: &mut ContextOf<Q>,
) -> Option<()>
where
    Q: Composition,
    KindsOf<Q>: Selector<K, I>,
    Q::Pack: PackNth<Q, KindsOf<Q>, I>,
{
    <KindPosAt<Q, I> as KindOps<Q, DescAt<Q, I>>>::ending(root, at, cx)
}
