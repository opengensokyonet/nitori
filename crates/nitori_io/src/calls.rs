//! Operations for family-based `CallOn`. Target methods are exported by the crate.
//!
//! Constructors only capture arguments; all checks and IO happen when polled.
//! A completed operation must not be polled again. Derived operations preserve
//! completed prefixes on Pending, errors, and cancellation; they never roll back.
use core::{ops::CoroutineState, task::Poll};

mod read;
mod write;

// The public operation facade keeps call types and their errors together.
pub use read::{
    EndianValue, Read, ReadArray, ReadArrayError, ReadBe, ReadBeError, ReadChunk, ReadChunks,
    ReadChunksError, ReadChunksExact, ReadChunksExactError, ReadExact, ReadExactError, ReadLe,
    ReadLeError, ReadToEnd, ReadToEndError,
};
pub use write::{Write, WriteAll, WriteAllError, WriteReturn};

type Step<Y, R> = Poll<CoroutineState<Y, R>>;
