//! A composition: its root kind, its parts and the pack of the positions of
//! every call, and the macro that writes one.
use core::marker::PhantomData;

use super::{ListShape, PackList, SlotsUnder};
use crate::{
    event::Outbox,
    structure::{End, KindFor},
};

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
