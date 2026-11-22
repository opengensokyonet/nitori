#![no_std]
#![feature(associated_type_defaults)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

extern crate alloc;

pub mod call;
pub mod capability;
pub mod hooks;
pub mod lend;
pub mod need;
pub mod structure;
