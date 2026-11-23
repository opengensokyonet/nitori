//! The places of a tree, the components at each, and the proof that every
//! hook of every component can be lent at its place.
//!
//! A place is a path type from the root ([`Place<X>`](Place)): every node
//! of the kind reached that way is there. The places follow from the
//! kinds alone ([`PlacesOf`]). At each place every part's share gives
//! components; each component's hooks are lent [`OwnNeeds`] at the place
//! and its `build` the [`Hooks::BuildNeeds`] at the parent's place, and its
//! subscriptions are resolved at the place. [`composition!`](crate::composition)
//! proves them all once, into the opaque [`DriverComposition::Places`].

#[cfg(doc)]
use crate::hooks::{Hooks, OwnNeeds};

mod component;
mod info;
mod list;
mod ops;
mod parts;
mod path;
mod prove;
mod share;

use core::marker::PhantomData;

use nitori_clock::MonotonicClock;

use crate::{
    call::{Composition, LendInstant, PartsOf, RootOf},
    hooks::PartList,
    request::{PlaceRequests, RequestSumOf},
    structure::{Address, End, Node, ParentPath, PathFrom},
};

pub use component::{BuildAt, CompLend, CompPositions, ProveComp};
pub use info::{CompInfo, PlaceInfo};
pub use list::{
    ChildPlaces, FindPlace, KindNotIn, KindsOnce, NoPlace, NthPlace, Place, PlaceDesc, PlaceKey,
    PlaceKeyOf, PlaceShape, PlacesOf, PlacesUnder,
};
pub use ops::{NoPlacePos, PlaceAt, PlaceNth, PlaceOps, PlacePack, PlacePositionsAt};
pub use parts::PartsOps;
pub use path::{ChildPath, Detach, EndBy, ParentPlace};
pub use prove::{
    ChildPlace, LinkPlace, NoLinkPlace, PlacePositions, ProveChildPlaces, ProveLinkPlace,
    ProveParts, ProvePlace, ProvePlaces, SameLinkKey, prove_places,
};
pub use share::ShareOps;

/// A list of parts whose arguments are for itself.
pub trait SelfParts: PartList<Self> + Sized + 'static {}
impl<L: PartList<L> + 'static> SelfParts for L {}

/// A composition a driver runs: besides the calls, every hook of every
/// component proved at its place, the driver's clock, and the sum of the
/// requests.
///
/// Written with [`composition!`](crate::composition)`(impl driver for Q:
/// Root, Parts, Clock)`; the places are opaque, defined by
/// [`places`](DriverComposition::places).
pub trait DriverComposition:
    Composition<
        Parts: SelfParts,
        Root: PlacesUnder<
            End,
            Out: PlaceShape<<Self as Composition>::Root>
                     + PlaceRequests<<Self as Composition>::Root>,
        >,
    >
{
    /// The clock the driver reads and sleeps on; every component with a
    /// deadline has its instant.
    type Clock: MonotonicClock<Instant = LendInstant<Self>> + 'static;
    /// What the parts keep of the root's shares after it ended
    /// ([`RemainsOf`](crate::structure::RemainsOf)`<Parts, Root>`).
    type Remains: Default + 'static;
    /// The positions of every hook of every place.
    type Places: PlacePack<Self, PlacesOf<Self>>;
    /// How what each call yields goes into the requests.
    type Yields: crate::driver::walk::YieldPack<Self, crate::call::CallSlotsOf<Self>>;
    /// Only to define `Places`.
    fn places() -> PhantomData<Self::Places>;
    /// Only to define `Yields`.
    fn yields() -> PhantomData<Self::Yields>;
}
/// The instant of the clock `C`.
pub type ClockInstantOf<C> = <C as MonotonicClock>::Instant;
/// The clock of `Q`.
pub type ClockOf<Q> = <Q as DriverComposition>::Clock;
/// The instant of the clock of `Q`.
pub type InstantOf<Q> = <ClockOf<Q> as MonotonicClock>::Instant;
/// The sum of the requests of `Q`'s tree.
pub type RequestsOf<Q> = RequestSumOf<RootOf<Q>>;
/// The root of `Q`'s tree.
pub type RootNode<Q> = Node<RootOf<Q>, PartsOf<Q>>;
/// The address of a node of `Q`'s tree.
pub type AddressOf<Q> = Address<RootOf<Q>>;
/// The kind at the path type `X` of `Q`'s tree.
pub type TargetOf<Q, X> = <X as PathFrom<RootOf<Q>>>::Target;
/// The path of the parent of the nodes at `X`.
pub type ParentOf<X> = <X as ParentPath>::Parent;
