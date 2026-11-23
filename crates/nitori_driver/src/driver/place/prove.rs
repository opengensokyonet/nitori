//! Proving a place: every component of every share of every part, and the
//! places of the children and of the link, into the place's positions.
//!
//! # What does not compile
//!
//! A hook's need not on the structure (the layer has no `Count`):
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Now, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Reads;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Reads {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!         _: &mut (),
//!     ) -> Self {
//!         Reads
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Reads];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A component of the root that reads a parent when it is built:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Now, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     type BuildNeeds = HList![At<Up<0>, Here, Ref<Count>>];
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! impl Share<Layer> for Part1 {
//!     type Data = HNil;
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
//!
//! A link whose target is kept under another type of key than the node
//! linking to it:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::time::Duration;
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     call::ShareCalls,
//! #     composition,
//! #     driver::Driver,
//! #     hooks::Part,
//! #     structure::{ByOwnKey, Child, LinkedStart, NodeKind, Share, SlotVec},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! struct Root;
//! struct Route;
//! struct Peer;
//! impl NodeKind for Root {
//!     type Children = HList![Child<Route, SlotVec<u32>>, Child<Peer, SlotVec<u64>>];
//!     type Linked = HNil;
//! }
//! impl NodeKind for Route {
//!     type Children = HNil;
//!     type Linked = HList![ByOwnKey<Peer>];
//! }
//! impl LinkedStart<Peer> for Route {
//!     fn linked_start(_: &mut ()) {}
//! }
//! impl NodeKind for Peer {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! # struct Base;
//! # impl<L> Part<L> for Base { type Args = (); }
//! # impl ShareCalls<Root> for Base { type Calls = HNil; }
//! # impl ShareCalls<Route> for Base { type Calls = HNil; }
//! # impl ShareCalls<Peer> for Base { type Calls = HNil; }
//! # impl Share<Root> for Base { type Data = HNil; }
//! # impl Share<Route> for Base { type Data = HNil; }
//! # impl Share<Peer> for Base { type Data = HNil; }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Base], Clock);
//! let _ = Driver::<Q>::new(Clock, frunk_core::hlist![()], ());
//! ```
//!
//! A kind at two places of the structure (the leaf under both `A` and `B`):
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::time::Duration;
//! # use frunk_core::{HList, hlist::HNil};
//! # use nitori_driver::{
//! #     call::ShareCalls,
//! #     composition,
//! #     hooks::Part,
//! #     structure::{AtMostOne, Child, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! struct Root;
//! struct A;
//! struct B;
//! struct Leaf;
//! impl NodeKind for Root {
//!     type Children = HList![Child<A, AtMostOne>, Child<B, AtMostOne>];
//!     type Linked = HNil;
//! }
//! impl NodeKind for A {
//!     type Children = HList![Child<Leaf, AtMostOne>];
//!     type Linked = HNil;
//! }
//! impl NodeKind for B {
//!     type Children = HList![Child<Leaf, AtMostOne>];
//!     type Linked = HNil;
//! }
//! impl NodeKind for Leaf {
//!     type Children = HNil;
//!     type Linked = HNil;
//! }
//! # struct Base;
//! # impl<L> Part<L> for Base { type Args = (); }
//! # impl ShareCalls<Root> for Base { type Calls = HNil; }
//! # impl ShareCalls<A> for Base { type Calls = HNil; }
//! # impl ShareCalls<B> for Base { type Calls = HNil; }
//! # impl ShareCalls<Leaf> for Base { type Calls = HNil; }
//! # impl Share<Root> for Base { type Data = HNil; }
//! # impl Share<A> for Base { type Data = HNil; }
//! # impl Share<B> for Base { type Data = HNil; }
//! # impl Share<Leaf> for Base { type Data = HNil; }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Base], Clock);
//! ```
//!
//! A subscription whose anchor is not on the way up:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type)]
//! # use core::{pin::Pin, task::{Context, Poll, Waker}, time::Duration};
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, Composition, LoanNeeds, ShareCalls},
//! #     composition,
//! #     driver::Driver,
//! #     event::{Hear, HearAt, NodeStarted},
//! #     hooks::{ArgsOf, BuildKeyed, BuildView, HasDeadline, HookKeys, HookView, Hooks, Part, PartList, Timed},
//! #     lend::KeyedList,
//! #     need::{At, Here, Kind, Mut, Now, Ref, Up},
//! #     structure::{AtMostOne, Child, End, NodeKind, Share},
//! # };
//! # #[derive(Clone, Default)]
//! # struct Clock;
//! # impl nitori_clock::MonotonicClock for Clock {
//! #     type Instant = Duration;
//! #     type Sleep = core::future::Pending<()>;
//! #     fn now(&self) -> Duration { Duration::ZERO }
//! #     fn checked_add(&self, at: Duration, by: Duration) -> Option<Duration> { at.checked_add(by) }
//! #     fn sleep_until(&self, _: Duration) -> Self::Sleep { core::future::pending() }
//! #     fn timer_granularity(&self) -> Duration { Duration::ZERO }
//! # }
//! # struct Root;
//! # struct Layer;
//! # struct Other;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Layer, AtMostOne>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Layer {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Other {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # struct Part1;
//! # impl<L> Part<L> for Part1 {
//! #     type Args = ();
//! # }
//! # impl ShareCalls<Root> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # impl ShareCalls<Layer> for Part1 {
//! #     type Calls = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u64);
//! # impl<L: PartList<L> + 'static> Hooks<Part1, L, Root> for Count {
//! #     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Root>>>(_: BuildView<'_, Self, Part1, L, Root, KP>, _: &ArgsOf<L>, _: &mut ()) -> Self {
//! #         Count(0)
//! #     }
//! # }
//! # impl Share<Root> for Part1 {
//! #     type Data = HList![Count];
//! # }
//! #[derive(Default)]
//! struct Hears;
//! impl<L: PartList<L> + 'static> Hooks<Part1, L, Layer> for Hears {
//!     type Subscribes = HList![HearAt<NodeStarted<Layer>, Kind<Other>, Here>];
//!     fn build<KP: KeyedList<L, BuildKeyed<Self, Part1, L, Layer>>>(
//!         _: BuildView<'_, Self, Part1, L, Layer, KP>,
//!         _: &ArgsOf<L>,
//!         _: &mut (),
//!     ) -> Self {
//!         Hears
//!     }
//! }
//! impl Share<Layer> for Part1 {
//!     type Data = HList![Hears];
//! }
//! struct Q;
//! composition!(impl driver for Q: Root, HList![Part1], Clock);
//! ```
use core::marker::PhantomData;

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};

use super::{FindPlace, Place, PlaceKey, PlaceKeyOf, PlacesOf, PlacesUnder, ProveComp, TargetOf};

use crate::{
    call::{Composition, PartsOf, RootOf},
    structure::{Append, Child, ChildStorage, End, NodeKind, PathFrom, Share, Step},
};

/// Every component of a share `Self` of the part `O` proved at `X`.
pub trait ProveShare<Q, X, O, I> {
    /// The positions, one per component.
    type Out;
}
impl<Q, X, O> ProveShare<Q, X, O, HNil> for HNil {
    type Out = HNil;
}
impl<Q, X, O, C: ProveComp<Q, X, O, IC>, T: ProveShare<Q, X, O, IT>, IC, IT>
    ProveShare<Q, X, O, HCons<IC, IT>> for HCons<C, T>
{
    type Out = HCons<C::Pos, T::Out>;
}
/// Every share of the parts `Self` proved at `X`.
pub trait ProveParts<Q, X, I> {
    /// The positions, one list per part.
    type Out;
}
impl<Q, X> ProveParts<Q, X, HNil> for HNil {
    type Out = HNil;
}
impl<Q, X, O, Ps, IO, IT> ProveParts<Q, X, HCons<IO, IT>> for HCons<O, Ps>
where
    Q: Composition,
    X: PathFrom<RootOf<Q>>,
    O: Share<TargetOf<Q, X>>,
    O::Data: ProveShare<Q, X, O, IO>,
    Ps: ProveParts<Q, X, IT>,
{
    type Out = HCons<<O::Data as ProveShare<Q, X, O, IO>>::Out, Ps::Out>;
}
/// The positions of a place: of every component (`PL`), the places of its
/// children (`Ix`, a list of [`ChildPlace`]), and the place its nodes link
/// to (`Lk`, [`LinkPlace`] or [`NoLinkPlace`]).
#[allow(clippy::type_complexity)]
pub struct PlacePositions<PL, Ix, Lk>(PhantomData<fn() -> (PL, Ix, Lk)>);
/// The place `Xc` of a child, its positions `Pc`, at the number `N` among
/// the places of the tree.
#[allow(clippy::type_complexity)]
pub struct ChildPlace<Xc, Pc, N>(PhantomData<fn() -> (Xc, Pc, N)>);
/// The place `Xb` the nodes of a place link to, its positions `Pb`, at the
/// number `N`.
#[allow(clippy::type_complexity)]
pub struct LinkPlace<Xb, Pb, N>(PhantomData<fn() -> (Xb, Pb, N)>);
/// The nodes of a place link to none.
pub struct NoLinkPlace;

/// The numbers of the places of the children of a node at `X` of `Q`'s
/// tree, from the child at `Pos` on; `I` is inferred.
pub trait ProveChildPlaces<Q, X, Pos, I> {
    /// A list of [`ChildPlace`].
    type Out;
}
impl<Q, X, Pos> ProveChildPlaces<Q, X, Pos, HNil> for HNil {
    type Out = HNil;
}
type AppendOf<X, Pos, H> = <X as Append<Pos, H>>::Out;
impl<Q, X, Pos, CK, S, T, N, IC, IT> ProveChildPlaces<Q, X, Pos, HCons<(N, IC), IT>>
    for HCons<Child<CK, S>, T>
where
    Q: Composition<Root: PlacesUnder<End>>,
    S: ChildStorage,
    X: Append<Pos, S::Handle>,
    PlacesOf<Q>: FindPlace<AppendOf<X, Pos, S::Handle>, N>,
    Place<AppendOf<X, Pos, S::Handle>>: ProvePlace<Q, IC>,
    T: ProveChildPlaces<Q, X, There<Pos>, IT>,
{
    type Out = HCons<
        ChildPlace<
            AppendOf<X, Pos, S::Handle>,
            <Place<AppendOf<X, Pos, S::Handle>> as ProvePlace<Q, IC>>::Pos,
            N,
        >,
        T::Out,
    >;
}
/// The number of the place the links (`Self`, its kind's
/// [`NodeKind::Linked`]) of the nodes at `X` point to, in `Q`'s tree; `I`
/// is inferred.
pub trait ProveLinkPlace<Q, X, I> {
    /// [`LinkPlace`] or [`NoLinkPlace`].
    type Out;
}
impl<Q, X> ProveLinkPlace<Q, X, ()> for HNil {
    type Out = NoLinkPlace;
}
type RootChildAt<Q, L> =
    Step<L, crate::structure::HandleAt<<RootOf<Q> as NodeKind>::Children, L>, End>;
/// A node links by its own key (`Self`) to a node kept under the key type
/// `K`: the two are the same.
#[diagnostic::on_unimplemented(
    message = "a node kept under `{Self}` links to a node kept under `{K}`",
    label = "the key types of a link differ",
    note = "the node linked to (`ByOwnKey`) is found by the linking node's own key: the node linked to must be kept under the same type of key as the node linking to it"
)]
pub trait SameLinkKey<K> {}
impl<K> SameLinkKey<K> for K {}
impl<Q, X, B, L, N, IB> ProveLinkPlace<Q, X, (L, N, IB)>
    for HCons<crate::structure::ByOwnKey<B>, HNil>
where
    Q: Composition<Root: PlacesUnder<End>>,
    B: NodeKind,
    <RootOf<Q> as NodeKind>::Children: crate::structure::ChildOfKind<B, L>,
    X: PlaceKey<RootOf<Q>>,
    <X as PlaceKey<RootOf<Q>>>::Key: SameLinkKey<PlaceKeyOf<Q, RootChildAt<Q, L>>>,
    PlacesOf<Q>: FindPlace<RootChildAt<Q, L>, N>,
    Place<RootChildAt<Q, L>>: ProvePlace<Q, IB>,
{
    type Out =
        LinkPlace<RootChildAt<Q, L>, <Place<RootChildAt<Q, L>> as ProvePlace<Q, IB>>::Pos, N>;
}

/// One place proved.
pub trait ProvePlace<Q, I> {
    /// Its positions.
    type Pos;
}
impl<Q: Composition, X, Ip, Ic, Il> ProvePlace<Q, (Ip, Ic, Il)> for Place<X>
where
    Q::Root: PlacesUnder<End>,
    X: PathFrom<RootOf<Q>>,
    PartsOf<Q>: ProveParts<Q, X, Ip>,
    <TargetOf<Q, X> as NodeKind>::Children: ProveChildPlaces<Q, X, I0, Ic>,
    <TargetOf<Q, X> as NodeKind>::Linked: ProveLinkPlace<Q, X, Il>,
{
    type Pos = PlacePositions<
        <PartsOf<Q> as ProveParts<Q, X, Ip>>::Out,
        <<TargetOf<Q, X> as NodeKind>::Children as ProveChildPlaces<Q, X, I0, Ic>>::Out,
        <<TargetOf<Q, X> as NodeKind>::Linked as ProveLinkPlace<Q, X, Il>>::Out,
    >;
}
/// Every place of a list proved (`I`, one variable for the tree); the
/// pack is the list of their positions.
pub trait ProvePlaces<Q, I> {
    /// The pack.
    type Out;
}
impl<Q> ProvePlaces<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, S: ProvePlace<Q, IS>, T: ProvePlaces<Q, IT>, IS, IT> ProvePlaces<Q, HCons<IS, IT>>
    for HCons<S, T>
{
    type Out = HCons<S::Pos, T::Out>;
}
/// The pack of the places `L` of `Q`; [`composition!`](crate::composition)
/// defines the opaque pack with it.
pub fn prove_places<Q, L: ProvePlaces<Q, I>, I>() -> PhantomData<L::Out> {
    PhantomData
}
