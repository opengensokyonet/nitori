//! The traits and every extension trait, for a single glob import:
//! `use nitori_codec::prelude::*;`.
//!
//! The extension traits are imported anonymously, so their methods are
//! available without their names entering scope.
pub use crate::{DecodeFrom, EncodeInto, EncodedLen};

pub use crate::{
    Decode as _, Encode as _,
    host::{DecodeHostExt as _, EncodeHostExt as _},
};
