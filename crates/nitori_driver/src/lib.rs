#![no_std]
#![feature(associated_type_defaults)]
#![cfg_attr(test, feature(impl_trait_in_assoc_type))]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
// The driver around the engine comes in later steps; until then, parts of
// the engine are used only by its tests.
#![allow(dead_code)]

extern crate alloc;
extern crate self as nitori_driver;

/// The lists and sums of frunk, which the structure, the needs and the
/// views are made of; the one re-export of it among the crates that use
/// this one.
pub use frunk_core;

pub mod call;
pub mod capability;
pub mod component;
pub mod driver;
pub mod id;
pub(crate) mod lend;
pub mod need;
pub mod react;
pub mod request;
pub mod structure;

/// What the examples of the documentation prove needs with. Not for use;
/// nothing here is stable.
#[doc(hidden)]
pub mod __private;
