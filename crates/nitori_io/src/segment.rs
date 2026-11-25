//! What one poll of a capability transferred, with the attributes of its
//! bytes or what accepting them produced.
use core::num::NonZeroUsize;

/// One segment that [`Read`](crate::Read) put into a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReadSegment<M> {
    /// The bytes put into the destination, which advanced by this count.
    pub count: NonZeroUsize,
    /// The attributes of all of these bytes.
    pub metadata: M,
}

/// One chunk that [`ReadChunk`](crate::ReadChunk) transferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkSegment<C, M> {
    /// The owned bytes, between one and the requested maximum.
    pub chunk: C,
    /// The attributes of all of the chunk's bytes.
    pub metadata: M,
}

/// One segment that [`Write`](crate::Write) accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WriteSegment<R> {
    /// The bytes accepted, by which the input's cursor advanced.
    pub count: NonZeroUsize,
    /// What accepting them produced.
    pub report: R,
}

/// One segment that [`WriteLent`](crate::WriteLent) kept from its lent
/// buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LentSegment<R, T> {
    /// The bytes the fill put into the lent buffer, all of which the sink
    /// kept.
    pub count: NonZeroUsize,
    /// What keeping them produced.
    pub report: R,
    /// The value the fill returned with them.
    pub value: T,
}
