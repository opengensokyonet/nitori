//! Operations derived from the capabilities, with their operation-specific
//! failures. The primitive operations are generated in [`crate::read`],
//! [`crate::read_chunk`] and [`crate::write`].
//!
//! Each `name` function is also an operation struct `Name` with conveniences:
//! `NameExt` on receiver accesses inside operations and `NameHostExt` on call
//! executors. Constructing an operation only captures its arguments; all checks
//! and IO happen when it is polled. Derived operations preserve completed
//! prefixes on Pending, errors, and cancellation; they never roll back.

mod read;
mod write;

pub use read::*;
pub use write::*;
