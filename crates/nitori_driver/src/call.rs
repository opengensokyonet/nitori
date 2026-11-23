//! Calls lent at nodes: their kinds, their family, and the composition
//! that proves every call of a tree once.
//!
//! A kind of call ([`CallKind`]) is the struct a `#[call]` function of
//! `nitori_call` generates: it gives the call's needs and its operation.
//! The function is generic over its family, `W: CallFamily<C = Kind>`;
//! each poll lends it a new view of its needs, which it reaches with
//! [`LoanNeeds::needs`] inside `host.with(…)`. Between polls a call keeps
//! only its own state.
//!
//! The family is [`Lent<Q, C, KP>`](Lent): the composition `Q`, the kind
//! `C`, and the positions `KP` of the call's keyed needs. Only the lender
//! names it; the call's code sees `KP` through the bound
//! [`CallFamily::KP`] carries, so it names no position.
//!
//! A composition ([`Composition`], written with [`composition!`](crate::composition!)) names a
//! root kind and a list of parts. The calls come from the parts' shares
//! ([`ShareCalls`]), node kind by node kind; every call's positions are
//! inferred once, in the composition's impl, into the opaque
//! [`Composition::Pack`]. [`start`] and [`poll`] reach a call of the pack
//! by its kind.
//!
//! A call on the root that takes every item by key, polled once:
//!
//! ```
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! /// On the root: every item by key, one poll.
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>, At<Up<0>, ToChild<Item, ByKey<u32>>, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| {
//!         let hlist_pat![own, items] = view.needs();
//!         for (_, c) in items.iter_mut() {
//!             own.0 += c.0;
//!         }
//!     });
//! }
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count];
//! }
//! impl Share<Item> for Part {
//!     type Data = HList![Count];
//! }
//! impl ShareCalls<Root> for Part {
//!     type Calls = HList![Counting];
//! }
//! impl ShareCalls<Item> for Part {
//!     type Calls = HNil;
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Part]);
//!
//! let mut root: Node<Root, HList![Part]> = Node::new(Default::default());
//! let _: ItemAt = insert(&mut root, End, 1, Node::new(frunk_core::hlist![frunk_core::hlist![Count(2)]])).ok().unwrap();
//! let mut call = core::pin::pin!(start::<Q, Counting, _, _>(&End));
//! let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
//! let step = poll::<Q, Counting, _, _>(&mut root, End, &nitori_driver::event::Outbox::new(), call.as_mut(), &mut cx);
//! assert!(matches!(step, Ok(CallStep::Finished)));
//! assert_eq!(root.data.head.head.0, 2);
//! ```
//!
//! # What does not compile
//!
//! A kind whose `Op` is the operation of another kind's function:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! struct Other;
//! impl<Q: Composition> CallKind<Q> for Other {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//!     type Op = Counting;
//!     fn start() -> Counting {
//!         Counting::new()
//!     }
//! }
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| view.needs().head.0 += 1);
//! }
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count];
//! }
//! impl Share<Item> for Part {
//!     type Data = HList![Count];
//! }
//! impl ShareCalls<Root> for Part {
//!     type Calls = HList![Counting, Other];
//! }
//! impl ShareCalls<Item> for Part {
//!     type Calls = HList![];
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Part]);
//! ```
//!
//! A function that yields what its kind does not say it yields:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Up<0>, Here, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call(yields = u32)]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     yield 1;
//! }
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count];
//! }
//! impl Share<Item> for Part {
//!     type Data = HList![Count];
//! }
//! impl ShareCalls<Root> for Part {
//!     type Calls = HList![Counting];
//! }
//! impl ShareCalls<Item> for Part {
//!     type Calls = HList![];
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Part]);
//! ```
//!
//! A call on an item that takes every item by key (`ByKey`), the store its own item is kept in:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Kind<Root>, ToChild<Item, ByKey<u32>>, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| {
//!         let hlist_pat![items] = view.needs();
//!         let _ = items.get_mut(&1);
//!     });
//! }
//! struct Part;
//! impl Share<Root> for Part {
//!     type Data = HList![Count];
//! }
//! impl Share<Item> for Part {
//!     type Data = HList![Count];
//! }
//! impl ShareCalls<Root> for Part {
//!     type Calls = HList![];
//! }
//! impl ShareCalls<Item> for Part {
//!     type Calls = HList![Counting];
//! }
//! struct Q;
//! composition!(impl for Q: Root, HList![Part]);
//! ```
//!
//! A call's body that takes a keyed view for the component, before any composition:
//!
//! ```compile_fail
//! # #![feature(impl_trait_in_assoc_type, yield_expr)]
//! # use frunk_core::{HList, hlist::HNil, hlist_pat};
//! # use nitori_call::{LentAccess, call};
//! # use nitori_driver::{
//! #     call::{CallFamily, CallKind, CallStep, Composition, LoanNeeds, ShareCalls, poll, start},
//! #     composition,
//! #     need::{At, ByKey, ByKeyOthers, Here, Kind, Mut, Up, ToChild},
//! #     structure::{Child, End, Node, NodeKind, Share, SlotVec, Step, insert},
//! # };
//! # struct Root;
//! # struct Item;
//! # impl NodeKind for Root {
//! #     type Children = HList![Child<Item, SlotVec<u32>>];
//! #     type Linked = HNil;
//! # }
//! # impl NodeKind for Item {
//! #     type Children = HNil;
//! #     type Linked = HNil;
//! # }
//! # #[derive(Default)]
//! # struct Count(u32);
//! # type ItemAt = Step<frunk_core::indices::Here, usize, End>;
//! impl<Q: Composition> CallKind<Q> for Counting {
//!     type Needs = HList![At<Kind<Root>, ToChild<Item, ByKeyOthers<u32>>, Mut<Count>>];
//!     type Op = Self;
//!     fn start() -> Self {
//!         Self::new()
//!     }
//! }
//! #[call]
//! async fn counting<W: CallFamily<C = Counting>>(host: &mut impl LentAccess<Family = W>) {
//!     host.with(|mut view| {
//!         let hlist_pat![others] = view.needs();
//!         others.0 = 1;
//!     });
//! }
//! ```
use alloc::vec::Vec;
use core::{
    any::TypeId,
    marker::PhantomData,
    ops::{Add, ControlFlow},
    pin::Pin,
    task::{Context, Poll},
};

use frunk_core::{
    hlist::{HCons, HNil},
    indices::{Here as I0, There},
};
use nitori_call::{Access, LentCall, LentSlot, Operation, ReceiverFamily, Start};

use crate::{
    event::{Outbox, Subscription, Subscriptions},
    lend::{
        Build, Cell, CellsOf, ChainCells, CxOf, Flat, Freeze, FrozenOf, KeyedList, KeyedPositions,
        LendError, LinkOf, LinkSpec, LinkedOf, Locate, NeedSet, Pairwise, Pass1, Pass2, Places,
        PositionsOf, Raw1Of, Raw2Of, Rem1Of, ViewOf, Writes,
    },
    need::ChainOf,
    structure::{Append, Child, ChildStorage, End, KindFor, Node, NodeKind, PathFrom},
};

// ---------------------------------------------------------------- kinds and the family

/// A kind of call lent at nodes, for every composition `Q`: its needs, and
/// the operation of its `#[call]` function.
///
/// The kind is the struct `#[call]` generates (`type Op = Self`), or a
/// marker naming it. The function is written for `W: CallFamily<C = the
/// kind>`; the composition checks that the operation runs on the family it
/// builds ([`RunsOn`]).
pub trait CallKind<Q: Composition>: 'static + Sized {
    /// The needs, an `HList` of needs.
    type Needs: NeedSet<PartsOf<Q>>;
    /// The operation the call runs.
    type Op;
    /// What the call yields to the driver; `Infallible` for a call that
    /// yields nothing.
    type Yield = core::convert::Infallible;
    /// What wakes the call: a list of [`Hear`](crate::event::Hear) and
    /// [`HearAt`](crate::event::HearAt), resolved where the call is lent.
    type Subscribes = HNil;
    /// The operation of a new call.
    fn start() -> Self::Op;
    /// Whether the call is no longer needed, read from the view of its
    /// needs before it is polled; the driver then drops it and does not
    /// poll it again. By default never.
    fn no_longer_needed<KP: Positions<Q, Self>>(view: &View<'_, Self, Q, KP>) -> bool {
        let _ = view;
        false
    }
}
/// The keyed needs of the call `C`, in order.
pub type KeyedOf<Q, C> = <<C as CallKind<Q>>::Needs as Flat>::Keyed;
/// The view of the call `C` under `Q`, its keyed needs at `KP`.
pub type View<'w, C, Q, KP> = ViewOf<'w, <C as CallKind<Q>>::Needs, PartsOf<Q>, KP>;

/// The positions of the keyed needs of the call `C` under `Q`; the view
/// reaches them through this bound only.
pub trait Positions<Q: Composition, C: CallKind<Q>>: KeyedList<PartsOf<Q>, KeyedOf<Q, C>> {}
impl<Q: Composition, C: CallKind<Q>, KP: KeyedList<PartsOf<Q>, KeyedOf<Q, C>>> Positions<Q, C>
    for KP
{
}

/// The family of the call `C` under `Q`, its keyed needs at `KP`.
#[allow(clippy::type_complexity)]
pub struct Lent<Q, C, KP>(PhantomData<fn() -> (Q, C, KP)>);

/// What a call is lent on one poll: the view of its needs.
pub struct Loan<'v, Q: Composition, C: CallKind<Q>, KP: Positions<Q, C>>(View<'v, C, Q, KP>);
// A loan holds borrows only; nothing in it is pinned structurally. The
// view's type has projections in it, so `Unpin` is not inferred.
impl<Q: Composition, C: CallKind<Q>, KP: Positions<Q, C>> Unpin for Loan<'_, Q, C, KP> {}

impl<Q: Composition, C: CallKind<Q>, KP: Positions<Q, C>> ReceiverFamily for Lent<Q, C, KP> {
    type ReceiverView<'v>
        = Loan<'v, Q, C, KP>
    where
        Self: 'v;
}

/// What a call's `#[call]` function is generic over: the composition, the
/// kind, and the keyed positions, whose bound the associated type carries.
pub trait CallFamily: ReceiverFamily + 'static {
    /// The composition.
    type Q: Composition;
    /// The kind of the call.
    type C: CallKind<Self::Q>;
    /// The positions of the call's keyed needs.
    type KP: Positions<Self::Q, Self::C>;
    /// The needs in this poll's view.
    #[allow(clippy::type_complexity)]
    fn needs<'s, 'v>(
        view: Pin<&'s mut Self::ReceiverView<'v>>,
    ) -> &'s mut View<'v, Self::C, Self::Q, Self::KP>
    where
        Self: 'v;
}
impl<Q: Composition, C: CallKind<Q>, KP: Positions<Q, C> + 'static> CallFamily for Lent<Q, C, KP> {
    type Q = Q;
    type C = C;
    type KP = KP;
    fn needs<'s, 'v>(view: Pin<&'s mut Loan<'v, Q, C, KP>>) -> &'s mut View<'v, C, Q, KP>
    where
        Self: 'v,
    {
        &mut view.get_mut().0
    }
}

/// The needs of a call's view, as `view.needs()` inside `host.with(|mut
/// view| …)`.
///
/// The view is lent for this poll and stays the loan's; destructuring it
/// gives `&mut` bindings to its items.
pub trait LoanNeeds<'h, W: CallFamily> {
    /// The needs in this poll's view.
    #[allow(clippy::type_complexity)]
    fn needs(&mut self) -> &mut View<'h, W::C, W::Q, W::KP>;
}
impl<'h, W: CallFamily> LoanNeeds<'h, W> for Access<'_, 'h, W> {
    fn needs(&mut self) -> &mut View<'h, W::C, W::Q, W::KP> {
        W::needs(self.as_pin())
    }
}

/// The operation `Self` runs on the family `F` as a call lent at a node,
/// yielding `Y`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not the operation of a `#[call]` function for the family `{F}` that yields `{Y}`",
    label = "the kind's `Op`",
    note = "a kind's `Op` is the struct its own `#[call]` function generates, the function written for `W: CallFamily<C = the kind>`, with `()` as its output and the kind's `Yield` as what it yields"
)]
pub trait RunsOn<F: ReceiverFamily, Y>:
    Operation<F, Output = (), Yield = Y> + for<'s> Start<'s, LentSlot<'s, F>>
{
}
impl<F: ReceiverFamily, Y, Op> RunsOn<F, Y> for Op where
    Op: Operation<F, Output = (), Yield = Y> + for<'s> Start<'s, LentSlot<'s, F>>
{
}

/// What one poll of a call came to.
#[derive(Debug, PartialEq, Eq)]
pub enum CallStep<Y> {
    /// The call waits.
    Pending,
    /// The call is over; it is not polled again.
    Finished,
    /// The call yielded an item and goes on.
    Yielded(Y),
    /// The call said it is no longer needed and was not polled; it is
    /// dropped.
    NoLongerNeeded,
}

// ---------------------------------------------------------------- the composition

/// A composition: a root kind, a list of parts, and the positions of every
/// call of the tree.
///
/// Written with [`composition!`](crate::composition!); the pack is opaque, defined by
/// [`pack`](Composition::pack).
pub trait Composition: 'static + Sized {
    /// The root kind.
    type Root: KindFor<Self::Parts> + SlotsUnder<Self::Parts, End, Slots: ListShape<Self>>;
    /// The list of parts.
    type Parts: 'static;
    /// The instant a lend is given ([`Now`](crate::need::Now)): the driver
    /// clock's for a composition a driver runs, `()` otherwise.
    type Instant: Copy + Ord + core::fmt::Debug + 'static;
    /// The positions of every call slot of the tree.
    type Pack: PackList<Self, CallSlotsOf<Self>>;
    /// Only to define `Pack`.
    fn pack() -> PhantomData<Self::Pack>;
}
/// The root kind of `Q`.
pub type RootOf<Q> = <Q as Composition>::Root;
/// The list of parts of `Q`.
pub type PartsOf<Q> = <Q as Composition>::Parts;
/// The instant a lend under `Q` is given.
pub type LendInstant<Q> = <Q as Composition>::Instant;
/// The outbox of a lend under `Q`, with its instant.
pub type OutboxOf<Q> = Outbox<LendInstant<Q>>;
/// Every call of the tree of `Q` with the path type of its nodes, in tree
/// order.
pub type CallSlotsOf<Q> = <RootOf<Q> as SlotsUnder<PartsOf<Q>, End>>::Slots;

/// Write a composition: `composition!(impl for Q: Root, Parts)`, or, for a
/// composition a [`Driver`](crate::driver::Driver) runs,
/// `composition!(impl driver for Q: Root, Parts, Clock)`, which also
/// proves every hook of every component at its place and checks that
/// every component with a deadline has the clock's instant.
///
/// The calls come from the parts' shares; every position is inferred by
/// the compiler inside the impl, once. The crate that writes it enables
/// the nightly feature `impl_trait_in_assoc_type`.
///
/// A kind at two places of the structure is refused by a check the
/// compiler evaluates after it infers the positions. When a component
/// returns a request for that kind, the inference fails first, as
/// `E0282 type annotations needed`: on seeing E0282, read the error after
/// it, the evaluation of that check, which names the kind.
#[macro_export]
macro_rules! composition {
    (impl driver for $q:ty: $r:ty, $p:ty, $clock:ty) => {
        $crate::composition!(@calls $q: $r, $p, $crate::driver::ClockInstantOf<$clock>);
        // a kind at two places of the structure fails here, naming it
        const _: () = <$crate::__private::PlacesOf<$q> as $crate::__private::KindsOnce<$r>>::CHECK;
        impl $crate::driver::DriverComposition for $q {
            type Clock = $clock;
            type Remains = $crate::structure::RemainsOf<$p, $r>;
            type Places = impl $crate::__private::PlacePack<Self, $crate::__private::PlacesOf<Self>>;
            type Yields = impl $crate::__private::YieldPack<Self, $crate::call::CallSlotsOf<Self>>;
            fn places() -> ::core::marker::PhantomData<Self::Places> {
                $crate::__private::prove_places::<Self, $crate::__private::PlacesOf<Self>, _>()
            }
            fn yields() -> ::core::marker::PhantomData<Self::Yields> {
                $crate::__private::prove_yields::<Self, $crate::call::CallSlotsOf<Self>, _>()
            }
        }
    };
    (impl for $q:ty: $r:ty, $p:ty) => {
        $crate::composition!(@calls $q: $r, $p, ());
    };
    (@calls $q:ty: $r:ty, $p:ty, $instant:ty) => {
        impl $crate::call::Composition for $q {
            type Root = $r;
            type Parts = $p;
            type Instant = $instant;
            type Pack = impl $crate::call::PackList<Self, $crate::call::CallSlotsOf<Self>>;
            fn pack() -> ::core::marker::PhantomData<Self::Pack> {
                $crate::call::prove::<Self, $crate::call::CallSlotsOf<Self>, _>()
            }
        }
    };
}

// ---------------------------------------------------------------- the calls of the shares

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

// ---------------------------------------------------------------- the pack

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

// ---------------------------------------------------------------- proving every slot

/// The positions of one slot: the link's, pass 1's, pass 2's, the keyed
/// needs' and the subscriptions'.
#[allow(clippy::type_complexity)]
pub struct SlotPositions<IL, I1, I2, IK, IS>(PhantomData<fn() -> (IL, I1, I2, IK, IS)>);

/// Every slot of a list proved, its positions inferred (`I`, one variable
/// for the whole tree); the pack is the list of the positions.
pub trait Prove<Q, I> {
    /// The pack.
    type Out;
}
impl<Q> Prove<Q, HNil> for HNil {
    type Out = HNil;
}
impl<Q, S, T, IS, IT> Prove<Q, HCons<IS, IT>> for HCons<S, T>
where
    S: ProveOne<Q, IS>,
    T: Prove<Q, IT>,
{
    type Out = HCons<S::Pos, T::Out>;
}
/// The pack of the slots `L` of `Q`; [`composition!`](crate::composition!) defines the opaque
/// pack with it.
pub fn prove<Q: Composition, L: Prove<Q, I>, I>() -> PhantomData<L::Out> {
    PhantomData
}

/// One slot proved: its call can be lent at its nodes under `Q`.
///
/// The positions are inferred at `'static`; they do not depend on the
/// lifetime, and the pack's bound proves them again for every lifetime.
#[diagnostic::on_unimplemented(
    message = "the call in `{Self}` cannot be lent at its node under `{Q}`",
    label = "a need of this call is not met",
    note = "a need's anchor, steps or component is not on this structure, two needs take the same place, a keyed need uses the word that does not fit its store (`ByKey` off the chain, `ByKeyOthers` on it), a part does not contribute to a family the call uses, or the kind's `Op` is not the operation of a `#[call]` function for this kind; `explain::<Q, Kind, Path, _, _, _, _>()` names the need"
)]
pub trait ProveOne<Q, I> {
    /// The slot's positions.
    type Pos;
}

type Needs<Q, C> = <C as CallKind<Q>>::Needs;
/// The chain up from the nodes at the path type `X` of `Q`'s tree.
pub type ChainAt<Q, X> = <X as ChainOf<RootOf<Q>, HNil>>::Chain;
type KPOf<Q, C, X, IK> = PositionsOf<Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IK>;

impl<Q, X, C, IL, IC, IP, IK, I1, I2, IS> ProveOne<Q, (IL, IC, IP, IK, I1, I2, IS)> for Slot<X, C>
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    LinkedOf<RootOf<Q>, X>: LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>,
    X: ChainCells<'static, RootOf<Q>, PartsOf<Q>, HNil, LinkOf<RootOf<Q>, PartsOf<Q>, X, IL>>,
    Needs<Q, C>: Locate<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IC,
        >,
    Needs<Q, C>: Places<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IP,
        >,
    <Needs<Q, C> as Places<
        CxOf<RootOf<Q>, PartsOf<Q>, X>,
        CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
        IP,
    >>::Out: Pairwise,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
    KPOf<Q, C, X, IK>: Positions<Q, C> + 'static,
    Needs<Q, C>: Pass1<
            'static,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            I1,
        >,
    Rem1Of<'static, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>: Freeze,
    Needs<Q, C>: Pass2<
            'static,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            FrozenOf<'static, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>,
            I2,
        >,
    C::Op: RunsOn<Lent<Q, C, KPOf<Q, C, X, IK>>, C::Yield>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>,
    Needs<Q, C>: Writes<PartsOf<Q>>,
{
    type Pos = SlotPositions<IL, I1, I2, IK, IS>;
}

/// The item-by-item check of the call of the kind `C` at the path type `X`
/// under `Q`, each bound on its own, so that the first error names the
/// failing need. It does nothing; call it to read why a composition does
/// not compile.
pub fn explain<Q, C, X, IL, IC, IP, IK>()
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    LinkedOf<RootOf<Q>, X>: LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>,
    X: ChainCells<'static, RootOf<Q>, PartsOf<Q>, HNil, LinkOf<RootOf<Q>, PartsOf<Q>, X, IL>>,
    Needs<Q, C>: Locate<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IC,
        >,
    Needs<Q, C>: Places<
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            IP,
        >,
    <Needs<Q, C> as Places<
        CxOf<RootOf<Q>, PartsOf<Q>, X>,
        CellsOf<'static, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
        IP,
    >>::Out: Pairwise,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
{
}

impl<Q, X, C, IL, I1, I2, IK, IS> SlotCall<Q, Slot<X, C>> for SlotPositions<IL, I1, I2, IK, IS>
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
    KPOf<Q, C, X, IK>: Positions<Q, C> + 'static,
    C::Op: RunsOn<Lent<Q, C, KPOf<Q, C, X, IK>>, C::Yield>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>,
    Needs<Q, C>: Writes<PartsOf<Q>>,
{
    type Family = Lent<Q, C, KPOf<Q, C, X, IK>>;
    type Running = LentCall<Self::Family, C::Op>;
    fn start(_: &X) -> Self::Running {
        LentCall::new(C::start())
    }
    fn subscriptions(out: &mut Vec<Subscription>) {
        <C::Subscribes as Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>>::subscriptions(out);
    }
    fn writes(out: &mut Vec<TypeId>) {
        <Needs<Q, C> as Writes<PartsOf<Q>>>::writes(out);
    }
}

impl<'a, Q, X, C, IL, I1, I2, IK, IS> LendOne<'a, Q, Slot<X, C>>
    for SlotPositions<IL, I1, I2, IK, IS>
where
    Q: Composition,
    C: CallKind<Q>,
    X: PathFrom<RootOf<Q>> + ChainOf<RootOf<Q>, HNil>,
    Needs<Q, C>: KeyedPositions<CxOf<RootOf<Q>, PartsOf<Q>, X>, IK>,
    KPOf<Q, C, X, IK>: Positions<Q, C> + 'static,
    C::Op: RunsOn<Lent<Q, C, KPOf<Q, C, X, IK>>, C::Yield>,
    C::Subscribes: Subscriptions<ChainAt<Q, X>, PartsOf<Q>, IS>,
    Needs<Q, C>: Writes<PartsOf<Q>>,
    LinkedOf<RootOf<Q>, X>: LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>,
    X: ChainCells<'a, RootOf<Q>, PartsOf<Q>, HNil, LinkOf<RootOf<Q>, PartsOf<Q>, X, IL>>,
    Needs<Q, C>: Pass1<
            'a,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            CellsOf<'a, RootOf<Q>, PartsOf<Q>, X, IL, LendInstant<Q>>,
            I1,
        >,
    Rem1Of<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>: Freeze,
    Needs<Q, C>: Pass2<
            'a,
            CxOf<RootOf<Q>, PartsOf<Q>, X>,
            FrozenOf<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>,
            I2,
        >,
    Needs<Q, C>: Build<
            'a,
            PartsOf<Q>,
            KeyedOf<Q, C>,
            KPOf<Q, C, X, IK>,
            Raw1Of<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, LendInstant<Q>>,
            Raw2Of<'a, Needs<Q, C>, RootOf<Q>, PartsOf<Q>, X, IL, I1, I2, LendInstant<Q>>,
        >,
{
    fn poll(
        root: &'a mut Node<RootOf<Q>, PartsOf<Q>>,
        at: X,
        outbox: &'a OutboxOf<Q>,
        call: Pin<&mut Self::Running>,
        cx: &mut Context<'_>,
    ) -> Result<CallStep<C::Yield>, LendError> {
        let link =
            <LinkedOf<RootOf<Q>, X> as LinkSpec<RootOf<Q>, PartsOf<Q>, X, IL>>::find(root, &at)?;
        let cells = HCons {
            head: Cell::new(outbox),
            tail: at.cells(root, link, crate::lend::FromSplit)?,
        };
        let (raw1, rem) =
            <Needs<Q, C> as Pass1<'a, CxOf<RootOf<Q>, PartsOf<Q>, X>, _, I1>>::take(cells);
        let frozen = rem.freeze();
        let raw2 = <Needs<Q, C> as Pass2<'a, CxOf<RootOf<Q>, PartsOf<Q>, X>, _, I2>>::get(&frozen);
        let mut loan: Loan<'a, Q, C, KPOf<Q, C, X, IK>> = Loan(<Needs<Q, C> as Build<
            'a,
            PartsOf<Q>,
            KeyedOf<Q, C>,
            KPOf<Q, C, X, IK>,
            _,
            _,
        >>::view(raw1, raw2));
        if C::no_longer_needed::<KPOf<Q, C, X, IK>>(&loan.0) {
            return Ok(CallStep::NoLongerNeeded);
        }
        Ok(match call.poll_with_views(Pin::new(&mut loan), cx) {
            Poll::Pending => CallStep::Pending,
            Poll::Ready(ControlFlow::Break(())) => CallStep::Finished,
            Poll::Ready(ControlFlow::Continue(item)) => CallStep::Yielded(item),
        })
    }
}
