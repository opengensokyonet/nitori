//! The call slots of a tree: the calls the shares give, a slot for each at
//! its path, and the lists of them.
use core::{marker::PhantomData, ops::Add};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::{CallKind, Composition};
use crate::structure::{Append, Child, ChildStorage, NodeKind};

/// The kinds of the calls the part `Self` keeps at each node of the kind
/// `K`, an `HList` (`HNil` for none).
pub trait ShareCalls<K> {
    /// The kinds of calls.
    type Calls;
}
/// The calls of a list of parts at the kind `K`, in part order.
pub trait PartsCalls<K> {
    /// The kinds of calls.
    type Calls;
}
impl<K> PartsCalls<K> for HNil {
    type Calls = HNil;
}
impl<K, Pt: ShareCalls<K>, T: PartsCalls<K>> PartsCalls<K> for HCons<Pt, T>
where
    Pt::Calls: Add<T::Calls>,
{
    type Calls = <Pt::Calls as Add<T::Calls>>::Output;
}

/// A call of the kind `C` at the nodes a path of the type `X` leads to.
pub struct Slot<X, C>(PhantomData<fn() -> (X, C)>);
/// The head of an empty list of slots.
pub struct NoSlot;
/// What a slot is lent at.
pub trait SlotDesc {
    /// The path type of its nodes.
    type At;
}
impl<X, C> SlotDesc for Slot<X, C> {
    type At = X;
}
impl SlotDesc for NoSlot {
    type At = core::convert::Infallible;
}
/// What the call of a slot yields under `Q`.
pub trait SlotYield<Q> {
    /// The kind's [`CallKind::Yield`].
    type Yield;
}
impl<Q: Composition, X, C: CallKind<Q>> SlotYield<Q> for Slot<X, C> {
    type Yield = C::Yield;
}
impl<Q> SlotYield<Q> for NoSlot {
    type Yield = core::convert::Infallible;
}

/// The calls `Self` put at the path type `X`.
pub trait AtPath<X> {
    /// The slots.
    type Out;
}
impl<X> AtPath<X> for HNil {
    type Out = HNil;
}
impl<X, C, T: AtPath<X>> AtPath<X> for HCons<C, T> {
    type Out = HCons<Slot<X, C>, T::Out>;
}
/// Every call slot of the kind `Self` at the path type `X` and below it,
/// for the parts `P`.
pub trait SlotsUnder<P, X> {
    /// The slots, in tree order.
    type Slots;
}
impl<K: NodeKind, P: PartsCalls<K>, X> SlotsUnder<P, X> for K
where
    P::Calls: AtPath<X>,
    K::Children: ChildSlots<P, X, I0>,
    <P::Calls as AtPath<X>>::Out: Add<<K::Children as ChildSlots<P, X, I0>>::Out>,
{
    type Slots =
        <<P::Calls as AtPath<X>>::Out as Add<<K::Children as ChildSlots<P, X, I0>>::Out>>::Output;
}
/// The call slots below a node at `X`, from its child at `Pos` on.
pub trait ChildSlots<P, X, Pos> {
    /// The slots.
    type Out;
}
impl<P, X, Pos> ChildSlots<P, X, Pos> for HNil {
    type Out = HNil;
}
impl<P, X, Pos, CK, S, T> ChildSlots<P, X, Pos> for HCons<Child<CK, S>, T>
where
    S: ChildStorage,
    X: Append<Pos, S::Handle>,
    CK: SlotsUnder<P, X::Out>,
    T: ChildSlots<P, X, There<Pos>>,
    CK::Slots: Add<T::Out>,
{
    type Out = <CK::Slots as Add<T::Out>>::Output;
}

/// A list of slots under `Q` as head and tail (the empty list: [`NoSlot`]
/// and itself).
pub trait ListShape<Q> {
    /// The first slot.
    type Head: SlotDesc + SlotYield<Q>;
    /// The others.
    type Tail: ListShape<Q>;
}
impl<Q> ListShape<Q> for HNil {
    type Head = NoSlot;
    type Tail = HNil;
}
impl<Q, H: SlotDesc + SlotYield<Q>, T: ListShape<Q>> ListShape<Q> for HCons<H, T> {
    type Head = H;
    type Tail = T;
}
/// The slot at the position `I` of a list.
pub trait NthItem<Q, I>: ListShape<Q> {
    /// The slot.
    type Item: SlotDesc + SlotYield<Q>;
}
impl<Q, L: ListShape<Q>> NthItem<Q, I0> for L {
    type Item = L::Head;
}
impl<Q, L: ListShape<Q>, I> NthItem<Q, There<I>> for L
where
    L::Tail: NthItem<Q, I>,
{
    type Item = <L::Tail as NthItem<Q, I>>::Item;
}
/// The slot of the call `C` is at the position `I`; a kind of call is in
/// at most one slot of a tree.
pub trait FindSlot<C, I> {
    /// The path type of its nodes.
    type At;
}
impl<X, C, T> FindSlot<C, I0> for HCons<Slot<X, C>, T> {
    type At = X;
}
impl<H, T: FindSlot<C, I>, C, I> FindSlot<C, There<I>> for HCons<H, T> {
    type At = T::At;
}
