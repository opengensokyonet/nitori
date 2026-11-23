#![no_std]
#![feature(associated_type_defaults)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

extern crate alloc;

/// The lists and sums of frunk, which the structure, the needs and the
/// views are made of; the one re-export of it among the crates that use
/// this one.
pub use frunk_core;

pub mod call;
pub mod capability;
pub mod driver;
pub mod event;
pub mod hooks;
pub mod lend;
pub mod need;
pub mod request;
pub mod structure;

/// What [`composition!`] and the bounds of the driver's items name: the
/// proofs of a composition and the steps they are made of. Not for use;
/// nothing here is stable.
#[doc(hidden)]
pub mod __private {
    pub use crate::driver::{
        edit::{
            Deliver, DriverOps, GrandchildPlace, GrandchildPlaces, LinkAdd, NoChildPlace,
            NoGrandchildPlace, NoPlaceEdits, PlaceEdit,
        },
        place::{
            BuildAt, ChildPath, ChildPlace, ChildPlaces, CompInfo, CompLend, CompPositions, Detach,
            FindPlace, LinkPlace, NoLinkPlace, NoPlace, NoPlacePos, NthPlace, ParentOf,
            ParentPlace, PartsOps, Place, PlaceAt, PlaceDesc, PlaceInfo, PlaceKey, PlaceKeyOf,
            PlaceNth, PlaceOps, PlacePack, PlacePositions, PlacePositionsAt, PlaceShape, PlacesOf,
            PlacesUnder, ProveChildPlaces, ProveComp, ProveLinkPlace, ProveParts, ProvePlace,
            ProvePlaces, SelfParts, ShareOps, prove_places,
        },
        walk::*,
    };
    pub use crate::lend::{
        cells::*, check::*, keyed::*, own_key::FindOwnKey, pass::*, view::*, writes::*,
    };
}
