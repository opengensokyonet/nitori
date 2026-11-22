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
pub mod hooks;
pub mod lend;
pub mod need;
pub mod structure;
