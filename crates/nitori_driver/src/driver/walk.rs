//! Reaching a place or a call slot by its number: the driver walks the
//! packs by number, so that no function's type names a whole pack.
use alloc::{boxed::Box, vec::Vec};
use core::{marker::PhantomData, pin::Pin};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::place::{
    DriverComposition, NthPlace, PlaceInfo, PlaceNth, PlaceOps, PlacesOf, RequestsOf,
};
use crate::{
    call::{
        CallSlotsOf, LendOne, ListShape, NthItem, PackNth, RootOf, RunningAt, SlotAt, SlotCall,
        SlotDesc, SlotYield,
    },
    event::Subscription,
    request::IntoRequestSum,
    structure::{Address, FromAddress},
};

/// The number a frunk index stands for.
pub trait Number {
    /// The number.
    const VALUE: usize;
}
impl Number for I0 {
    const VALUE: usize = 0;
}
impl<I: Number> Number for There<I> {
    const VALUE: usize = 1 + I::VALUE;
}

/// The list from the position `I` on is empty ([`Yes`]) or not ([`No`]).
pub trait EmptyFrom<I> {
    /// [`Yes`] or [`No`].
    type Empty;
}
/// The list from a position on is empty.
pub struct Yes;
/// The list from a position on is not empty.
pub struct No;
impl EmptyFrom<I0> for HNil {
    type Empty = Yes;
}
impl<H, T> EmptyFrom<I0> for HCons<H, T> {
    type Empty = No;
}
impl<H, T: EmptyFrom<I>, I> EmptyFrom<There<I>> for HCons<H, T> {
    type Empty = T::Empty;
}

/// Walks a pack from the number `I` on.
pub struct Cursor<I>(PhantomData<fn() -> I>);

// ---------------------------------------------------------------- places

/// Something done at a place, whose type is known only when the walk
/// reaches it.
pub trait PlaceVisit<Q: DriverComposition> {
    /// What it comes to.
    type Out;
    /// Do it at the place `Pl`, whose positions are `Pos`.
    fn visit<Pl: super::place::PlaceDesc<RootOf<Q>>, Pos: PlaceOps<Q, Pl>>(self) -> Self::Out;
}
/// The places of `Q` from the number `I` on.
pub trait PlaceWalk<Q: DriverComposition> {
    /// The facts of every place, in order.
    fn info(out: &mut Vec<PlaceInfo>);
    /// Do `v` at the place `p`; `None` past the last place.
    fn visit<V: PlaceVisit<Q>>(p: usize, v: V) -> Option<V::Out>;
}
/// One step of [`PlaceWalk`], by whether the list is empty from `I` on.
pub trait PlaceStep<Q: DriverComposition, I> {
    /// See [`PlaceWalk::info`].
    fn info(out: &mut Vec<PlaceInfo>);
    /// See [`PlaceWalk::visit`].
    fn visit<V: PlaceVisit<Q>>(p: usize, v: V) -> Option<V::Out>;
}
impl<Q: DriverComposition, I> PlaceWalk<Q> for Cursor<I>
where
    PlacesOf<Q>: EmptyFrom<I>,
    <PlacesOf<Q> as EmptyFrom<I>>::Empty: PlaceStep<Q, I>,
{
    fn info(out: &mut Vec<PlaceInfo>) {
        <<PlacesOf<Q> as EmptyFrom<I>>::Empty as PlaceStep<Q, I>>::info(out)
    }
    fn visit<V: PlaceVisit<Q>>(p: usize, v: V) -> Option<V::Out> {
        <<PlacesOf<Q> as EmptyFrom<I>>::Empty as PlaceStep<Q, I>>::visit(p, v)
    }
}
impl<Q: DriverComposition, I> PlaceStep<Q, I> for Yes {
    fn info(_: &mut Vec<PlaceInfo>) {}
    fn visit<V: PlaceVisit<Q>>(_: usize, _: V) -> Option<V::Out> {
        None
    }
}
impl<Q: DriverComposition, I: Number> PlaceStep<Q, I> for No
where
    PlacesOf<Q>: NthPlace<RootOf<Q>, I>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I>,
    Cursor<There<I>>: PlaceWalk<Q>,
{
    fn info(out: &mut Vec<PlaceInfo>) {
        out.push(
            <<Q::Places as PlaceNth<Q, PlacesOf<Q>, I>>::Out as PlaceOps<
                Q,
                <PlacesOf<Q> as NthPlace<RootOf<Q>, I>>::Item,
            >>::info(),
        );
        <Cursor<There<I>> as PlaceWalk<Q>>::info(out);
    }
    fn visit<V: PlaceVisit<Q>>(p: usize, v: V) -> Option<V::Out> {
        if p == I::VALUE {
            Some(v.visit::<<PlacesOf<Q> as NthPlace<RootOf<Q>, I>>::Item, <Q::Places as PlaceNth<
                Q,
                PlacesOf<Q>,
                I,
            >>::Out>())
        } else {
            <Cursor<There<I>> as PlaceWalk<Q>>::visit(p, v)
        }
    }
}

// ---------------------------------------------------------------- call slots

/// The running calls of one slot, by node number, and what they yielded
/// and the driver has not handled yet; `A` is the address of the tree.
pub struct SlotStore<Run, Y, A> {
    /// The call at each node number, while it runs.
    pub calls: Vec<Option<Pin<Box<Run>>>>,
    /// What the calls yielded, in order.
    pub yields: Vec<Y>,
    _addr: PhantomData<fn() -> A>,
}
impl<Run, Y, A> Default for SlotStore<Run, Y, A> {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            yields: Vec::new(),
            _addr: PhantomData,
        }
    }
}

/// A call slot, as the driver keeps it.
#[derive(Debug)]
pub struct SlotInfo {
    /// The path type of its nodes.
    pub place: core::any::TypeId,
    /// What wakes it.
    pub subscriptions: Vec<Subscription>,
    /// The components its needs take exclusive.
    pub writes: Vec<core::any::TypeId>,
}

/// Something done at a call slot, with the slot's store.
pub trait SlotVisit<Q: DriverComposition> {
    /// What it comes to.
    type Out;
    /// Do it at the slot `S`, whose positions are `Pos`.
    fn visit<S, Pos>(
        self,
        store: &mut SlotStore<Pos::Running, S::Yield, Address<RootOf<Q>>>,
    ) -> Self::Out
    where
        S: SlotDesc<At: FromAddress<RootOf<Q>> + 'static> + SlotYield<Q>,
        Pos: SlotCall<Q, S> + for<'a> LendOne<'a, Q, S>;
}
/// The call slots of `Q` from the number of `Self` on, with their stores.
pub trait CallWalk<Q: DriverComposition> {
    /// The stores of the slots from here on.
    type Store: Default + frunk_core::hlist::HList;
    /// The facts of every slot, in order.
    fn info(out: &mut Vec<SlotInfo>);
    /// Do `v` at the slot `s`; `None` past the last slot.
    fn visit<V: SlotVisit<Q>>(s: usize, store: &mut Self::Store, v: V) -> Option<V::Out>;
    /// What the calls of the slot `s` yielded, taken out into the
    /// requests.
    fn take_requests(s: usize, store: &mut Self::Store, out: &mut Vec<RequestsOf<Q>>);
}
/// One step of [`CallWalk`], by whether the list is empty from `I` on.
pub trait SlotStep<Q: DriverComposition, I> {
    /// See [`CallWalk::Store`].
    type Store: Default + frunk_core::hlist::HList;
    /// See [`CallWalk::info`].
    fn info(out: &mut Vec<SlotInfo>);
    /// See [`CallWalk::visit`].
    fn visit<V: SlotVisit<Q>>(s: usize, store: &mut Self::Store, v: V) -> Option<V::Out>;
    /// See [`CallWalk::take_requests`].
    fn take_requests(s: usize, store: &mut Self::Store, out: &mut Vec<RequestsOf<Q>>);
}
impl<Q: DriverComposition, I> CallWalk<Q> for Cursor<I>
where
    CallSlotsOf<Q>: EmptyFrom<I>,
    <CallSlotsOf<Q> as EmptyFrom<I>>::Empty: SlotStep<Q, I>,
{
    type Store = <<CallSlotsOf<Q> as EmptyFrom<I>>::Empty as SlotStep<Q, I>>::Store;
    fn info(out: &mut Vec<SlotInfo>) {
        <<CallSlotsOf<Q> as EmptyFrom<I>>::Empty as SlotStep<Q, I>>::info(out)
    }
    fn visit<V: SlotVisit<Q>>(s: usize, store: &mut Self::Store, v: V) -> Option<V::Out> {
        <<CallSlotsOf<Q> as EmptyFrom<I>>::Empty as SlotStep<Q, I>>::visit(s, store, v)
    }
    fn take_requests(s: usize, store: &mut Self::Store, out: &mut Vec<RequestsOf<Q>>) {
        <<CallSlotsOf<Q> as EmptyFrom<I>>::Empty as SlotStep<Q, I>>::take_requests(s, store, out)
    }
}
impl<Q: DriverComposition, I> SlotStep<Q, I> for Yes {
    type Store = HNil;
    fn info(_: &mut Vec<SlotInfo>) {}
    fn visit<V: SlotVisit<Q>>(_: usize, _: &mut HNil, _: V) -> Option<V::Out> {
        None
    }
    fn take_requests(_: usize, _: &mut HNil, _: &mut Vec<RequestsOf<Q>>) {}
}
type YieldAt<Q, I> = <SlotAt<Q, I> as SlotYield<Q>>::Yield;
/// The store of the slot at `I`.
pub type SlotStoreAt<Q, I> = SlotStore<RunningAt<Q, I>, YieldAt<Q, I>, Address<RootOf<Q>>>;
impl<Q: DriverComposition, I: Number> SlotStep<Q, I> for No
where
    CallSlotsOf<Q>: NthItem<Q, I, Item: SlotDesc<At: FromAddress<RootOf<Q>> + 'static>>,
    Q::Pack: PackNth<Q, CallSlotsOf<Q>, I>,
    Q::Yields: YieldNth<Q, CallSlotsOf<Q>, I>,
    Cursor<There<I>>: CallWalk<Q>,
{
    type Store = HCons<SlotStoreAt<Q, I>, <Cursor<There<I>> as CallWalk<Q>>::Store>;
    fn info(out: &mut Vec<SlotInfo>) {
        let mut subscriptions = Vec::new();
        <<Q::Pack as PackNth<Q, CallSlotsOf<Q>, I>>::Out as SlotCall<Q, SlotAt<Q, I>>>::subscriptions(
            &mut subscriptions,
        );
        let mut writes = Vec::new();
        <<Q::Pack as PackNth<Q, CallSlotsOf<Q>, I>>::Out as SlotCall<Q, SlotAt<Q, I>>>::writes(
            &mut writes,
        );
        out.push(SlotInfo {
            place: core::any::TypeId::of::<<SlotAt<Q, I> as SlotDesc>::At>(),
            subscriptions,
            writes,
        });
        <Cursor<There<I>> as CallWalk<Q>>::info(out);
    }
    fn visit<V: SlotVisit<Q>>(s: usize, store: &mut Self::Store, v: V) -> Option<V::Out> {
        if s == I::VALUE {
            Some(
                v.visit::<SlotAt<Q, I>, <Q::Pack as PackNth<Q, CallSlotsOf<Q>, I>>::Out>(
                    &mut store.head,
                ),
            )
        } else {
            <Cursor<There<I>> as CallWalk<Q>>::visit(s, &mut store.tail, v)
        }
    }
    fn take_requests(s: usize, store: &mut Self::Store, out: &mut Vec<RequestsOf<Q>>) {
        if s == I::VALUE {
            out.extend(
                store
                    .head
                    .yields
                    .drain(..)
                    .map(<Q::Yields as YieldNth<Q, CallSlotsOf<Q>, I>>::convert),
            );
        } else {
            <Cursor<There<I>> as CallWalk<Q>>::take_requests(s, &mut store.tail, out)
        }
    }
}

// ---------------------------------------------------------------- yields

/// How what the call of each slot of the list `L` yields goes into the
/// requests of `Q`.
pub trait YieldPack<Q: DriverComposition, L: ListShape<Q>> {
    /// The other slots'.
    type Tail: YieldPack<Q, L::Tail>;
    /// What the first slot's call yielded, in the requests.
    fn convert(y: <L::Head as SlotYield<Q>>::Yield) -> RequestsOf<Q>;
}
impl<Q: DriverComposition> YieldPack<Q, HNil> for HNil {
    type Tail = HNil;
    fn convert(y: core::convert::Infallible) -> RequestsOf<Q> {
        match y {}
    }
}
impl<Q, S, L, J, T> YieldPack<Q, HCons<S, L>> for HCons<J, T>
where
    Q: DriverComposition,
    S: SlotDesc + SlotYield<Q>,
    S::Yield: IntoRequestSum<RequestsOf<Q>, J>,
    L: ListShape<Q>,
    T: YieldPack<Q, L>,
{
    type Tail = T;
    fn convert(y: S::Yield) -> RequestsOf<Q> {
        y.into_sum()
    }
}
/// The conversion of what the slot at `I` yields, through the pack's
/// bounds only.
pub trait YieldNth<Q: DriverComposition, L: NthItem<Q, I>, I> {
    /// What the slot's call yielded, in the requests.
    fn convert(y: <L::Item as SlotYield<Q>>::Yield) -> RequestsOf<Q>;
}
impl<Q: DriverComposition, L: ListShape<Q>, P: YieldPack<Q, L>> YieldNth<Q, L, I0> for P {
    fn convert(y: <L::Head as SlotYield<Q>>::Yield) -> RequestsOf<Q> {
        P::convert(y)
    }
}
impl<Q: DriverComposition, L: ListShape<Q>, P: YieldPack<Q, L>, I> YieldNth<Q, L, There<I>> for P
where
    L::Tail: NthItem<Q, I>,
    P::Tail: YieldNth<Q, L::Tail, I>,
{
    fn convert(y: <<L::Tail as NthItem<Q, I>>::Item as SlotYield<Q>>::Yield) -> RequestsOf<Q> {
        <P::Tail as YieldNth<Q, L::Tail, I>>::convert(y)
    }
}
/// Every slot of a list proved to yield what goes into the requests of
/// `Q` (`I`, one variable for the list); the pack is the list of their
/// positions.
pub trait ProveYields<Q, I> {
    /// The pack.
    type Out;
}
impl<Q> ProveYields<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, S, L, J, IT> ProveYields<Q, HCons<J, IT>> for HCons<S, L>
where
    Q: DriverComposition,
    S: SlotYield<Q>,
    S::Yield: IntoRequestSum<RequestsOf<Q>, J>,
    L: ProveYields<Q, IT>,
{
    type Out = HCons<J, L::Out>;
}
/// The pack of the slots `L` of `Q`; [`composition!`](crate::composition)
/// defines the opaque pack with it.
pub fn prove_yields<Q, L: ProveYields<Q, I>, I>() -> PhantomData<L::Out> {
    PhantomData
}
