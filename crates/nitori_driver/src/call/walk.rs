//! Walking the pack by position, for the tests: each entry names the
//! kind (and call or component) it works on by the types alone.

use core::{
    pin::Pin,
    task::{self, Poll},
};

use frunk_core::{
    hlist::{HCons, HNil, Selector},
    indices::{Here, There},
};

use super::{
    Composition, ContextOf, KindsOf, PartsOf, PriorityOf, RootNode,
    host::CallHost,
    kinds::ListRest,
    pack::{CallList, CompList, EndReasonOf, KindDesc, KindList, KindOps, PackList, StartOf},
};
use crate::component::{ArgsOf, EndCause};

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
    host: &mut CallHost<PriorityOf<Q>>,
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
        root, at, cx, call, host, wcx,
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
