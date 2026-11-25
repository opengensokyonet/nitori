//! Proving a composition once: for every kind of its structure, the
//! positions of every call's needs, of every component's operations, of
//! its build and of the driver's reaction points, inferred together into
//! the pack.
//!
//! The positions are inferred where the composition is written
//! ([`composition!`](crate::composition!)), by the bounds of [`Prove`];
//! what they make is the pack's items, whose operations ([`KindOps`],
//! [`CallList`], [`CompList`]) lend at them without inferring again.
use alloc::boxed::Box;
use core::{
    marker::PhantomData,
    pin::Pin,
    task::{self, Poll},
};

use frunk_core::hlist::{HCons, HNil};
use nitori_call::LentCall;

use super::{
    CallKind, Comp, Composition, ContextOf, KindsOf, Lent, Loan, PartsOf, PriorityOf, RootNode,
    RootOf, RunsOn,
    host::CallHost,
    kinds::{KindsOnce, ListRest},
    pack::{CallList, CompList, Desc, FkOf, IlOf, KindDesc, KindList, KindOps, PackList, SlotPoll},
};
use crate::{
    capability::contribution::ListedKnown,
    component::{ArgsOf, Component, EndCause},
    driver::{ChildOpsList, ChildPos, Core, Edit, FkPlace, HandlerAt},
    id::{NumberValue, Numbered, number_of},
    lend::list::{S, Z},
    lend::{
        build::{NewPointCx, NewPointFk},
        entry::{
            self, BuildRootShares, BuildShares, EndingReactions, Lends, OwnNeeds, StartedReactions,
            build_node, build_root,
        },
        tree::{Ctx, InKid, IsSelf, Locate},
    },
    need::Pos,
    react::{NodeEnding, NodeStarted},
    request::HandleRequest,
    structure::{Child, ChildrenOf, KindFor, MoveError, Node, NodeIdent, NodeKind, OccupiedError},
};

/// The most calls a kind of node has: the driver keeps a bit for each in
/// one word.
type MaxCalls = S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<S<Z>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>;

/// A list with at most the budget `B` of items.
pub trait Fits<B> {}
impl<B> Fits<B> for HNil {}
impl<H, T: Fits<B>, B> Fits<S<B>> for HCons<H, T> {}

/// The calls `Self` of one kind of node are few enough for the driver.
#[diagnostic::on_unimplemented(
    message = "a kind of node has more than 64 calls",
    label = "the calls of the shares at one kind",
    note = "the driver keeps a bit for each call of a node in one word; split the calls between kinds, or join some of them into one call"
)]
pub trait FewCalls {}
impl<L: Fits<MaxCalls>> FewCalls for L {}

/// The route from the root of `Q` to the kind `K`.
type RouteOf<Q, K> = <RootOf<Q> as Locate<K, IlOf<Q, K>>>::Route;

/// What was proved of one kind: the positions of its calls `PC`, of its
/// components `PO`, of its build `IB`, of the reaction points started
/// `IS` and ending `IE`, of its handler's needs `IH`, and of its
/// children's kinds `PK`.
#[allow(clippy::type_complexity)]
pub struct KindPositions<PC, PO, IB, IS, IE, IH, PK>(
    PhantomData<fn() -> (PC, PO, IB, IS, IE, IH, PK)>,
);
/// The needs of the handler of `K` under `Q`.
type HandlerNeeds<Q, K> = <<K as NodeKind>::Handler as HandleRequest<K, PriorityOf<Q>>>::Needs;

// ------------------------------------------------------------------ proving

/// Every kind of a list proved, the positions inferred (`I`, one variable
/// for the whole structure); the pack is the list of what was proved.
pub trait Prove<Q, I> {
    /// The pack.
    type Out;
}
impl<Q> Prove<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, K, T, IK, IT> Prove<Q, HCons<IK, IT>> for HCons<K, T>
where
    K: ProveKind<Q, IK>,
    T: Prove<Q, IT>,
{
    type Out = HCons<K::Pos, T::Out>;
}
/// The pack of the kinds `L` of `Q`; [`composition!`](crate::composition!)
/// defines the opaque pack with it. Every item of the composition's list of
/// contributions names one of its parts.
pub fn prove<Q: Composition, L: Prove<Q, I> + KindsOnce, I>() -> PhantomData<L::Out>
where
    Q::Contributions: ListedKnown<PartsOf<Q>>,
{
    PhantomData
}
/// The pack `P` with its tables; [`composition!`](crate::composition!)
/// defines the opaque tables with it.
pub fn with_tables<Q: Composition, L: KindList<Q>, P: PackList<Q, L>>(
    _: PhantomData<P>,
) -> PhantomData<(P, P::Tables)> {
    PhantomData
}

/// One kind of the structure proved under `Q`: every call and component
/// of it can be lent at its nodes, its nodes can be built, and the driver
/// can fire its reaction points there.
#[diagnostic::on_unimplemented(
    message = "the node kind `{Self}` cannot be composed under `{Q}`",
    note = "every call of every share at the kind is lent its needs at its nodes, every component its own needs and its build needs, and the driver fires `NodeStarted` and `NodeEnding` there; one of them is not met"
)]
pub trait ProveKind<Q, I> {
    /// What was proved.
    type Pos;
}
type CallsOfDesc<Q, K> = <Desc<K> as KindDesc<Q>>::Calls;
type CompsOfDesc<Q, K> = <Desc<K> as KindDesc<Q>>::Comps;
impl<Q, K, IC, IO, IB, IS, IE, IH, IK> ProveKind<Q, (IC, IO, IB, IS, IE, IH, IK)> for K
where
    Q: Composition,
    K: KindFor<PartsOf<Q>>,
    Desc<K>: KindDesc<Q, Kind = K>,
    CallsOfDesc<Q, K>: ProveCalls<Q, K, IC> + FewCalls,
    CompsOfDesc<Q, K>: ProveComps<Q, K, IO>,
    IlOf<Q, K>: BuildAt<Q, K, IB>,
    NodeStarted<K>: Numbered,
    NodeEnding<K>: Numbered,
    IS: Pos,
    IE: Pos,
    StartedReactions<K>: Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, IS>,
    EndingReactions<K>: Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, IE>,
    K::Handler: HandleRequest<K, PriorityOf<Q>> + ProveHandler<Q, K, IH>,
    ChildrenOf<K>: ProveChildren<Q, IK>,
{
    type Pos = KindPositions<
        <CallsOfDesc<Q, K> as ProveCalls<Q, K, IC>>::Out,
        <CompsOfDesc<Q, K> as ProveComps<Q, K, IO>>::Out,
        IB,
        IS,
        IE,
        IH,
        <ChildrenOf<K> as ProveChildren<Q, IK>>::Out,
    >;
}

/// The handler `Self` of the kind `K` can be lent its needs at the
/// position `P` (inferred) at the nodes of `K` under `Q`.
#[diagnostic::on_unimplemented(
    message = "the handler `{Self}` cannot be lent its needs at the nodes of the kind `{K}` under `{Q}`",
    label = "the handler of a kind of the structure",
    note = "an edit handle lends the handler's `Needs` at the node the request is for: one of them is not on this structure, or two take the same component"
)]
pub trait ProveHandler<Q, K, P> {}
impl<Q, K, H, P> ProveHandler<Q, K, P> for H
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    H: HandleRequest<K, PriorityOf<Q>>,
    P: Pos,
    H::Needs: Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, P>,
{
}

/// The kinds of the children `Self` found among `Q`'s kinds, at the
/// positions `I` (inferred).
pub trait ProveChildren<Q, I> {
    /// Their positions.
    type Out;
}
impl<Q> ProveChildren<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, CK, S, T, IC, IT> ProveChildren<Q, HCons<IC, IT>> for HCons<Child<CK, S>, T>
where
    Q: Composition,
    KindsOf<Q>: frunk_core::hlist::Selector<CK, IC>,
    T: ProveChildren<Q, IT>,
{
    type Out = HCons<ChildPos<IC>, T::Out>;
}

/// The call `Self` can be lent its needs at the position `P` (inferred)
/// at the nodes of the kind `K` under `Q`, and runs there.
#[diagnostic::on_unimplemented(
    message = "the call `{Self}` cannot be lent at the nodes of the kind `{K}` under `{Q}`",
    label = "a call of a share at this kind",
    note = "a need of the call is not on this structure, two of its needs take the same component, the context has no piece it reads, or the kind's `Op` is not the operation of its own `#[call]` function"
)]
pub trait ProveCall<Q, K, P> {}
impl<Q, K, C, P> ProveCall<Q, K, P> for C
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    C: CallKind<Q, K>,
    P: Pos,
    C::Needs: Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, P>,
    C::Op: RunsOn<Lent<Q, K, C, P>>,
{
}
/// Every call of the list `Self` proved at the kind `K`.
pub trait ProveCalls<Q, K, I> {
    /// Their positions.
    type Out;
}
impl<Q, K> ProveCalls<Q, K, HNil> for HNil {
    type Out = HNil;
}
impl<Q, K, C, Ct, P, It> ProveCalls<Q, K, HCons<P, It>> for HCons<C, Ct>
where
    Q: Composition,
    K: NodeKind,
    C: CallKind<Q, K> + ProveCall<Q, K, P>,
    Ct: ProveCalls<Q, K, It>,
{
    type Out = HCons<P, Ct::Out>;
}

/// The component `Self` of the part `O` can be lent itself and its needs
/// at the position `P` (inferred) at the nodes of the kind `K` under `Q`.
#[diagnostic::on_unimplemented(
    message = "the component `{Self}` of the part `{O}` cannot be lent at the nodes of the kind `{K}` under `{Q}`",
    note = "a component's `wake` and `end` are lent the component, exclusive, then its `Needs`, at its node: one of them is not on this structure, or two take the same component"
)]
pub trait ProveComponent<Q, K, O, P> {}
impl<Q, K, O, C, P> ProveComponent<Q, K, O, P> for C
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    C: Component<O, PartsOf<Q>, K>,
    P: Pos,
    OwnNeeds<C, O, PartsOf<Q>, K>: Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, P>,
{
}
/// Every component of the list `Self` proved at the kind `K`.
pub trait ProveComps<Q, K, I> {
    /// Their positions.
    type Out;
}
impl<Q, K> ProveComps<Q, K, HNil> for HNil {
    type Out = HNil;
}
impl<Q, K, O, C, Ct, P, It> ProveComps<Q, K, HCons<P, It>> for HCons<Comp<O, C>, Ct>
where
    C: ProveComponent<Q, K, O, P>,
    Ct: ProveComps<Q, K, It>,
{
    type Out = HCons<P, Ct::Out>;
}

// ------------------------------------------------------------------ building

/// Building a new node of the kind `K` under `Q`, `Self` being the way
/// down to it, with the positions `IB` of the build needs (inferred).
#[diagnostic::on_unimplemented(
    message = "the nodes of the kind `{K}` cannot be built under `{Q}`",
    note = "every component of a new node is lent its `BuildNeeds` from the nodes above it, and the root's components have none"
)]
pub trait BuildAt<Q: Composition, K: KindFor<PartsOf<Q>>, IB> {
    /// What a new node is built at.
    type At;
    /// The shares of a new node at `at`.
    fn build(
        root: &mut RootNode<Q>,
        at: &Self::At,
        start: &mut K::Start,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<<K as KindFor<PartsOf<Q>>>::Data>;
}
/// The positions of the root's build: none, since nothing is above it.
pub struct RootBuild;
impl<Q: Composition, K: KindFor<PartsOf<Q>>> BuildAt<Q, K, RootBuild> for IsSelf
where
    PartsOf<Q>: BuildRootShares<K, PartsOf<Q>, Data = <K as KindFor<PartsOf<Q>>>::Data>,
{
    type At = ();
    fn build(
        _: &mut RootNode<Q>,
        _: &(),
        start: &mut K::Start,
        args: &ArgsOf<PartsOf<Q>>,
        _: &mut ContextOf<Q>,
    ) -> Option<<K as KindFor<PartsOf<Q>>>::Data> {
        Some(build_root::<K, PartsOf<Q>>(start, args))
    }
}
impl<Q: Composition, K: KindFor<PartsOf<Q>>, I, Ps> BuildAt<Q, K, Ps> for InKid<I>
where
    RootOf<Q>: Locate<K, InKid<I>>,
    NewPointCx<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>: Ctx,
    PartsOf<Q>: BuildShares<
            RootOf<Q>,
            PartsOf<Q>,
            K,
            InKid<I>,
            ContextOf<Q>,
            NewPointFk<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>,
            Ps,
            Data = <K as KindFor<PartsOf<Q>>>::Data,
        >,
{
    type At = NewPointFk<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>>;
    fn build(
        root: &mut RootNode<Q>,
        at: &Self::At,
        start: &mut K::Start,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<<K as KindFor<PartsOf<Q>>>::Data> {
        build_node::<RootOf<Q>, PartsOf<Q>, K, InKid<I>, ContextOf<Q>, Ps>(
            root, at, start, args, cx,
        )
    }
}

// ------------------------------------------------------------------ what was proved, used

impl<Q, K, PC, PO, IB, IS, IE, IH, PK> KindOps<Q, Desc<K>>
    for KindPositions<PC, PO, IB, IS, IE, IH, PK>
where
    Q: Composition,
    K: KindFor<PartsOf<Q>>,
    Desc<K>: KindDesc<Q, Kind = K, Data = <K as KindFor<PartsOf<Q>>>::Data>,
    PC: CallList<Q, Desc<K>, CallsOfDesc<Q, K>>,
    PO: CompList<Q, Desc<K>, CompsOfDesc<Q, K>>,
    IlOf<Q, K>: BuildAt<Q, K, IB, At = <Desc<K> as KindDesc<Q>>::NewAt>,
    NodeStarted<K>: Numbered,
    NodeEnding<K>: Numbered,
    IS: Pos,
    IE: Pos,
    StartedReactions<K>:
        Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, IS, Fk = FkOf<Q, K>>,
    EndingReactions<K>:
        Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, IE, Fk = FkOf<Q, K>>,
    RootOf<Q>: Locate<K, IlOf<Q, K>>,
    (RootOf<Q>, RouteOf<Q, K>): FkPlace<PartsOf<Q>, FkOf<Q, K>, Kind0 = RootOf<Q>, Target = K>,
    K::Handler: HandleRequest<K, PriorityOf<Q>>,
    IH: Pos,
    HandlerNeeds<Q, K>:
        Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, IH, Fk = FkOf<Q, K>>,
    PK: ChildOpsList<Q, FkOf<Q, K>, ChildrenOf<K>>,
{
    type Calls = PC;
    type Comps = PO;
    type Children = PK;
    #[allow(private_interfaces)]
    fn handle(core: &mut Core<Q>, number: u32, fk: FkOf<Q, K>, request: K::Request) {
        let edit = Edit::<K, HandlerAt<Q, K, Self, IH>>::new(core, number, fk);
        <K::Handler as HandleRequest<K, PriorityOf<Q>>>::handle(edit, request);
    }
    fn ident(root: &RootNode<Q>, at: &FkOf<Q, K>) -> Option<NodeIdent> {
        <(RootOf<Q>, RouteOf<Q, K>) as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::ident(root, at)
    }
    fn transplant(
        root: &mut RootNode<Q>,
        from: &FkOf<Q, K>,
        to: &FkOf<Q, K>,
    ) -> Result<NodeIdent, MoveError> {
        type Place<Q, K> = (RootOf<Q>, RouteOf<Q, K>);
        match <Place<Q, K> as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::vacant(root, to) {
            None => return Err(MoveError::NoDestination),
            Some(false) => {
                return Err(MoveError::Occupied {
                    source: OccupiedError,
                });
            }
            Some(true) => {}
        }
        let node = <Place<Q, K> as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::remove(root, from)
            .ok_or(MoveError::NoSource)?;
        match <Place<Q, K> as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::insert(root, to, node) {
            Ok(parent) => Ok(parent),
            Err(node) => {
                // Invariant: the new parent was there and kept nothing
                // under the key just above, and taking the node out of
                // another parent changed neither; it goes back anyway.
                let _ = <Place<Q, K> as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::insert(root, from, node);
                Err(MoveError::NoDestination)
            }
        }
    }
    fn build_at(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        start: &mut K::Start,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<<K as KindFor<PartsOf<Q>>>::Data> {
        let at = <Desc<K> as KindDesc<Q>>::new_at(at);
        <IlOf<Q, K> as BuildAt<Q, K, IB>>::build(root, &at, start, args, cx)
    }
    fn vacant(root: &RootNode<Q>, at: &FkOf<Q, K>) -> Option<bool> {
        <(RootOf<Q>, RouteOf<Q, K>) as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::vacant(root, at)
    }
    fn insert(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        data: <K as KindFor<PartsOf<Q>>>::Data,
        number: u32,
    ) -> Result<(NodeIdent, NodeIdent), <K as KindFor<PartsOf<Q>>>::Data> {
        let mut node = Node::<K, PartsOf<Q>>::new(data);
        node.ident.number = number;
        let own = node.ident;
        <(RootOf<Q>, RouteOf<Q, K>) as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::insert(root, at, node)
            .map(|parent| (own, parent))
            .map_err(|node| node.data)
    }
    fn remove(root: &mut RootNode<Q>, at: &FkOf<Q, K>) -> bool {
        <(RootOf<Q>, RouteOf<Q, K>) as FkPlace<PartsOf<Q>, FkOf<Q, K>>>::remove(root, at).is_some()
    }
    fn build(
        root: &mut RootNode<Q>,
        at: &<Desc<K> as KindDesc<Q>>::NewAt,
        start: &mut K::Start,
        args: &ArgsOf<PartsOf<Q>>,
        cx: &mut ContextOf<Q>,
    ) -> Option<<K as KindFor<PartsOf<Q>>>::Data> {
        <IlOf<Q, K> as BuildAt<Q, K, IB>>::build(root, at, start, args, cx)
    }
    fn started(root: &mut RootNode<Q>, at: &FkOf<Q, K>, cx: &mut ContextOf<Q>) -> Option<()> {
        entry::node_started::<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, IS, ContextOf<Q>>(root, at, cx)
    }
    fn ending(root: &mut RootNode<Q>, at: &FkOf<Q, K>, cx: &mut ContextOf<Q>) -> Option<()> {
        entry::node_ending::<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, IE, ContextOf<Q>>(root, at, cx)
    }
}

impl<Q, K, C, Ct, P, PT> CallList<Q, Desc<K>, HCons<C, Ct>> for HCons<P, PT>
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    Ct: ListRest,
    C: CallKind<Q, K>,
    P: Pos,
    C::Needs: Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, P, Fk = FkOf<Q, K>>,
    C::Op: RunsOn<Lent<Q, K, C, P>> + 'static,
    PT: CallList<Q, Desc<K>, Ct>,
{
    type HeadRunning = LentCall<Lent<Q, K, C, P>, C::Op>;
    type Tail = PT;
    type Slots = HCons<Option<Pin<Box<Self::HeadRunning>>>, PT::Slots>;
    const COUNT: u32 = PT::COUNT + 1;
    fn start_head(start: &mut K::Start) -> Self::HeadRunning {
        LentCall::new(C::start(start))
    }
    fn poll_head(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        cx: &mut ContextOf<Q>,
        call: Pin<&mut Self::HeadRunning>,
        host: &mut CallHost<PriorityOf<Q>>,
        wcx: &mut task::Context<'_>,
    ) -> Option<Poll<()>> {
        // The loan holds a copy of what the driver keeps of the call, so
        // that it borrows nothing beside the view; it is written back
        // after the poll.
        let lent = host.clone();
        let (polled, lent) =
            <C::Needs as Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, P>>::lend_to(
                root,
                at,
                cx,
                move |view| {
                    let mut loan = Loan::<Q, K, C, P> { view, host: lent };
                    let polled = call.poll_return_with_views(Pin::new(&mut loan), wcx);
                    (polled, loan.host)
                },
            )?;
        *host = lent;
        Some(polled)
    }
    fn start_slots(start: &mut K::Start) -> Self::Slots {
        let head = Some(Box::pin(Self::start_head(start)));
        HCons {
            head,
            tail: PT::start_slots(start),
        }
    }
    fn poll_slot(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        cx: &mut ContextOf<Q>,
        slots: &mut Self::Slots,
        j: u32,
        host: &mut CallHost<PriorityOf<Q>>,
        wcx: &mut task::Context<'_>,
    ) -> SlotPoll {
        let Some(rest) = j.checked_sub(1) else {
            let Some(call) = &mut slots.head else {
                return SlotPoll::NoCall;
            };
            return match Self::poll_head(root, at, cx, call.as_mut(), host, wcx) {
                None => SlotPoll::NotThere,
                Some(Poll::Ready(())) => {
                    slots.head = None;
                    SlotPoll::Returned
                }
                Some(Poll::Pending) => SlotPoll::Pending,
            };
        };
        PT::poll_slot(root, at, cx, &mut slots.tail, rest, host, wcx)
    }
}

impl<Q, K, O, C, Ct, PT, P, PR> CompList<Q, Desc<K>, HCons<Comp<O, C>, Ct>>
    for HCons<HCons<PT, P>, PR>
where
    Q: Composition,
    K: NodeKind,
    Desc<K>: KindDesc<Q, Kind = K>,
    Ct: ListRest,
    C: Component<O, PartsOf<Q>, K> + Numbered<Number: NumberValue>,
    PT: Pos,
    P: Pos,
    OwnNeeds<C, O, PartsOf<Q>, K>:
        Lends<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, ContextOf<Q>, HCons<PT, P>, Fk = FkOf<Q, K>>,
    PR: CompList<Q, Desc<K>, Ct>,
{
    type Tail = PR;
    fn wake_head(root: &mut RootNode<Q>, at: &FkOf<Q, K>, cx: &mut ContextOf<Q>) -> Option<()> {
        entry::wake::<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, O, C, PT, P, ContextOf<Q>>(root, at, cx)
    }
    fn end_head(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        cx: &mut ContextOf<Q>,
        cause: EndCause<'_, K::EndReason>,
    ) -> Option<()> {
        entry::end::<RootOf<Q>, PartsOf<Q>, K, IlOf<Q, K>, O, C, PT, P, ContextOf<Q>>(
            root, at, cx, cause,
        )
    }
    fn wake_number(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        cx: &mut ContextOf<Q>,
        number: u64,
    ) -> Option<Option<()>> {
        if number_of::<C>() == number {
            return Some(Self::wake_head(root, at, cx));
        }
        PR::wake_number(root, at, cx, number)
    }
    fn end_all(
        root: &mut RootNode<Q>,
        at: &FkOf<Q, K>,
        cx: &mut ContextOf<Q>,
        cause: EndCause<'_, K::EndReason>,
    ) {
        // A node not in the tree has no components to end.
        let _ = Self::end_head(root, at, cx, cause);
        PR::end_all(root, at, cx, cause);
    }
}
