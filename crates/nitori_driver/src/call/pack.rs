//! The pack of a composition's calls: starting and polling the call of a
//! kind.
use alloc::vec::Vec;
use core::{any::TypeId, pin::Pin, task::Context};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::{
    CallFamily, CallKind, CallSlotsOf, CallStep, Composition, FindSlot, Lent, ListShape, NoSlot,
    NthItem, OutboxOf, PartsOf, RootOf, Slot, SlotDesc, SlotYield,
};
use crate::{event::Subscription, lend::LendError, structure::Node};

/// The call of the slot `S` under `Q`: its family, and the call as it runs
/// between polls.
pub trait SlotCall<Q: Composition, S: SlotDesc> {
    /// The call's family.
    type Family: CallFamily<Q = Q>;
    /// The call between polls.
    type Running;
    /// A new call for the node at `at`.
    fn start(at: &S::At) -> Self::Running;
    /// What the call subscribes to, resolved at its node.
    fn subscriptions(out: &mut Vec<Subscription>);
    /// The components its needs take exclusive.
    fn writes(out: &mut Vec<TypeId>);
}
/// Polling the slot `S`'s call under `Q` with a view lent for this poll.
pub trait LendOne<'a, Q: Composition, S: SlotDesc + SlotYield<Q>>: SlotCall<Q, S> {
    /// Lend the node at `at` and poll `call` once; what the view publishes
    /// goes to `outbox`.
    fn poll(
        root: &'a mut Node<RootOf<Q>, PartsOf<Q>>,
        at: S::At,
        outbox: &'a OutboxOf<Q>,
        call: Pin<&mut Self::Running>,
        cx: &mut Context<'_>,
    ) -> Result<CallStep<S::Yield>, LendError>;
}
/// The positions of every slot of the list `L` under `Q`.
pub trait PackList<Q: Composition, L: ListShape<Q>> {
    /// The positions of the first slot.
    type Head: SlotCall<Q, L::Head> + for<'a> LendOne<'a, Q, L::Head>;
    /// The positions of the others.
    type Tail: PackList<Q, L::Tail>;
}
/// The end of a pack.
pub struct NoPos;
/// The kind of the end of a pack: no needs, no operation.
pub struct NoCall;
impl<Q: Composition> CallKind<Q> for NoCall {
    type Needs = HNil;
    type Op = ();
    fn start() {}
}
impl<Q: Composition> SlotCall<Q, NoSlot> for NoPos {
    type Family = Lent<Q, NoCall, HNil>;
    type Running = core::convert::Infallible;
    fn start(at: &core::convert::Infallible) -> core::convert::Infallible {
        *at
    }
    fn subscriptions(_: &mut Vec<Subscription>) {}
    fn writes(_: &mut Vec<TypeId>) {}
}
impl<Q: Composition> LendOne<'_, Q, NoSlot> for NoPos {
    fn poll(
        _: &mut Node<RootOf<Q>, PartsOf<Q>>,
        at: core::convert::Infallible,
        _: &OutboxOf<Q>,
        _: Pin<&mut core::convert::Infallible>,
        _: &mut Context<'_>,
    ) -> Result<CallStep<core::convert::Infallible>, LendError> {
        match at {}
    }
}
impl<Q: Composition> PackList<Q, HNil> for HNil {
    type Head = NoPos;
    type Tail = HNil;
}
impl<Q: Composition, S: SlotDesc + SlotYield<Q>, L: ListShape<Q>, PH, PT> PackList<Q, HCons<S, L>>
    for HCons<PH, PT>
where
    PH: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>,
    PT: PackList<Q, L>,
{
    type Head = PH;
    type Tail = PT;
}
/// The positions of the `I`-th slot of a pack, through the pack's bounds
/// only: a driver walks the pack by slot number.
pub trait PackNth<Q: Composition, L: NthItem<Q, I>, I> {
    /// The positions.
    type Out: SlotCall<Q, L::Item> + for<'a> LendOne<'a, Q, L::Item>;
}
impl<Q: Composition, L: ListShape<Q>, P: PackList<Q, L>> PackNth<Q, L, I0> for P {
    type Out = P::Head;
}
impl<Q: Composition, L: ListShape<Q>, P: PackList<Q, L>, I> PackNth<Q, L, There<I>> for P
where
    L::Tail: NthItem<Q, I>,
    P::Tail: PackNth<Q, L::Tail, I>,
{
    type Out = <P::Tail as PackNth<Q, L::Tail, I>>::Out;
}

/// The slot at the position `I` of `Q`'s tree.
pub type SlotAt<Q, I> = <CallSlotsOf<Q> as NthItem<Q, I>>::Item;
/// The positions of the slot at `I`.
pub type PositionsAt<Q, I> = <<Q as Composition>::Pack as PackNth<Q, CallSlotsOf<Q>, I>>::Out;
/// The running call of the slot at `I`.
pub type RunningAt<Q, I> = <PositionsAt<Q, I> as SlotCall<Q, SlotAt<Q, I>>>::Running;

/// Start the call of the kind `C` for the node at `at`; `C` is found by its
/// kind among the tree's calls (`X` and `I` are written `_`).
pub fn start<Q, C, X, I>(at: &X) -> RunningAt<Q, I>
where
    Q: Composition,
    C: CallKind<Q>,
    CallSlotsOf<Q>: FindSlot<C, I, At = X> + NthItem<Q, I, Item = Slot<X, C>>,
    Q::Pack: PackNth<Q, CallSlotsOf<Q>, I>,
{
    <PositionsAt<Q, I> as SlotCall<Q, Slot<X, C>>>::start(at)
}
/// Lend the node at `at` to the call of the kind `C` and poll it once;
/// what its view publishes goes to `outbox`.
pub fn poll<'a, Q, C, X, I>(
    root: &'a mut Node<RootOf<Q>, PartsOf<Q>>,
    at: X,
    outbox: &'a OutboxOf<Q>,
    call: Pin<&mut RunningAt<Q, I>>,
    cx: &mut Context<'_>,
) -> Result<CallStep<C::Yield>, LendError>
where
    Q: Composition,
    C: CallKind<Q>,
    CallSlotsOf<Q>: FindSlot<C, I, At = X> + NthItem<Q, I, Item = Slot<X, C>>,
    Q::Pack: PackNth<Q, CallSlotsOf<Q>, I>,
{
    <PositionsAt<Q, I> as LendOne<'a, Q, Slot<X, C>>>::poll(root, at, outbox, call, cx)
}
