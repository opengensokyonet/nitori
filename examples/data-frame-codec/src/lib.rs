#![feature(impl_trait_in_assoc_type, yield_expr)]

//! DATA-frame codec example built on `nitori_call` and `nitori_io`.
//!
//! The codec demonstrates QUIC VarInt and single-frame HTTP/3 DATA
//! processing; it does not implement complete HTTP/3 semantics.
//!
//! The package also holds codec examples (`examples/`), behavioral tests
//! (`tests/`), compiler boundary probes (`tests/ui` and
//! `scripts/check_boundaries.py`) and macro expansion tooling
//! (`scripts/expand_example.py`, with its output in `expanded/`).

pub mod current_codec;
#[cfg(test)]
mod helper_tests;
