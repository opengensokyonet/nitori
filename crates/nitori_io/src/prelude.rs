//! The capabilities and every generated extension trait,
//! for a single glob import: `use nitori_io::prelude::*;`.
//!
//! The extension traits are imported anonymously, so their methods are
//! available without their names entering scope.
pub use crate::{Read, ReadChunk, Write};

pub use crate::{
    PollReadExt as _, PollWriteExt as _, ReadChunkExt as _, ReadChunkHostExt as _, ReadExt as _,
    ReadHostExt as _, WriteExt as _, WriteHostExt as _,
};

pub use crate::calls::{
    ReadArrayExt as _, ReadArrayHostExt as _, ReadBeExt as _, ReadBeHostExt as _,
    ReadChunksExactExt as _, ReadChunksExactHostExt as _, ReadChunksExt as _,
    ReadChunksHostExt as _, ReadExactExt as _, ReadExactHostExt as _, ReadLeExt as _,
    ReadLeHostExt as _, ReadToEndExt as _, ReadToEndHostExt as _, WriteAllExt as _,
    WriteAllHostExt as _, WriteArrayExt as _, WriteArrayHostExt as _, WriteBeExt as _,
    WriteBeHostExt as _, WriteLeExt as _, WriteLeHostExt as _,
};
