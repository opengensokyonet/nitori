//! A source written out as the bytes of a value, one wrapper for each
//! pairing of the source's and the sink's capabilities, as the copies of
//! `nitori_io::calls` pair them.
//!
//! Wrap the source to choose how its bytes reach the sink: [`Chunks`] when
//! the source hands out owned chunks ([`ReadChunk`]) and the sink accepts
//! bytes ([`Write`]), [`Fill`] when the sink lends its storage
//! ([`WriteLent`]) to a source that reads into it ([`Read`]), and
//! [`Buffered`], through a buffer of the caller's, when both only read and
//! accept bytes. A bare source is not a value: which of the three it would
//! take cannot be told from it, and a crate that keeps a source in a type of
//! its own encodes that type as it chooses.
//!
//! Each wrapper copies until the source ends, so bound the source to the
//! bytes it is to contribute first, such as with
//! [`ReadLimit`](nitori_io::adapters::ReadLimit); [`EncodedLen`] is the
//! source's exact remaining length ([`ExactLen`]).
//!
//! As in the copies, the bytes are written with the metadata they were read
//! with as their write options: the sink takes the source's metadata
//! type, `()` for a source without metadata. Compose
//! [`MappedOptions`](nitori_io::adapters::MappedOptions), with a
//! [`MapOptions`](nitori_io::adapters::MapOptions) mapping or
//! [`FixedOptions`](nitori_io::adapters::FixedOptions), on a sink that takes
//! other options.
use crate::{EncodeInto, EncodedLen};
use core::num::NonZeroUsize;
use nitori_call::ReceiverAccess;
use nitori_io::{
    Combine, ExactLen, ExactLenExt as _, Read, ReadChunk, Write, WriteLent,
    calls::{
        CopyBufferedError, CopyChunksError, CopyIntoLentError, CopyTotal, copy_buffered,
        copy_chunks, copy_into_lent,
    },
};
use snafu::{OptionExt as _, ResultExt as _, Snafu};

/// A source written out by writing its chunks, of at most `chunk_maximum`
/// bytes each: a source that hands out owned chunks ([`ReadChunk`]) into a
/// sink that accepts bytes ([`Write`]). Shared chunks, such as `Bytes`,
/// reach the sink without being copied.
#[derive(Clone, Copy, Debug)]
pub struct Chunks<A> {
    /// The source.
    pub source: A,
    /// The largest chunk to take from the source at once.
    pub chunk_maximum: NonZeroUsize,
}

/// A source written out by reading it into storage the sink lends
/// ([`WriteLent`]), at most `chunk_maximum` bytes for each loan: a source
/// that reads into a buffer ([`Read`]) without an intermediate copy.
#[derive(Clone, Copy, Debug)]
pub struct Fill<A> {
    /// The source.
    pub source: A,
    /// The largest loan to ask the sink for at once.
    pub chunk_maximum: NonZeroUsize,
}

/// A source written out through `buffer`: a source that reads ([`Read`])
/// into a sink that accepts bytes ([`Write`]), when neither lends nor hands
/// out chunks. An empty buffer fails with no progress possible.
#[derive(Clone, Copy, Debug)]
pub struct Buffered<A, B> {
    /// The source.
    pub source: A,
    /// The buffer the bytes pass through.
    pub buffer: B,
}

impl<A, S, M> EncodeInto<S> for Chunks<A>
where
    A: ReceiverAccess<Family: ReadChunk<Metadata = M>>,
    M: Combine,
    S: ReceiverAccess<Family: Write<M, Report: Combine>>,
{
    type Output = CopyTotal<M, <S::Family as Write<M>>::Report>;
    type Error = CopyChunksError<<A::Family as Read>::Error, <S::Family as Write<M>>::Error>;
    async fn encode_into(mut self, mut sink: S) -> Result<Self::Output, Self::Error> {
        copy_chunks(&mut self.source, &mut sink, self.chunk_maximum).await
    }
}

impl<A, S, M> EncodeInto<S> for Fill<A>
where
    A: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine + Clone,
    S: ReceiverAccess<Family: WriteLent<M>>,
    <S::Family as Write<M>>::Report: Combine,
{
    type Output = CopyTotal<M, <S::Family as Write<M>>::Report>;
    type Error = CopyIntoLentError<<A::Family as Read>::Error, <S::Family as Write<M>>::Error>;
    async fn encode_into(mut self, mut sink: S) -> Result<Self::Output, Self::Error> {
        copy_into_lent(&mut self.source, &mut sink, self.chunk_maximum).await
    }
}

impl<A, B, S, M> EncodeInto<S> for Buffered<A, B>
where
    A: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine,
    B: AsMut<[u8]>,
    S: ReceiverAccess<Family: Write<M, Report: Combine>>,
{
    type Output = CopyTotal<M, <S::Family as Write<M>>::Report>;
    type Error = CopyBufferedError<<A::Family as Read>::Error, <S::Family as Write<M>>::Error>;
    async fn encode_into(mut self, mut sink: S) -> Result<Self::Output, Self::Error> {
        copy_buffered(&mut self.source, &mut sink, self.buffer.as_mut()).await
    }
}

/// Why the length of a source written out as a value cannot be told.
#[derive(Debug, Snafu)]
#[snafu(module(source_length_error), visibility(pub(crate)))]
pub enum SourceLengthError<E> {
    /// The source cannot tell its length.
    #[snafu(display("the source cannot tell its length"))]
    Source { source: E },
    /// The source's length does not fit in 64 bits, which only a `Length`
    /// wider than 64 bits can reach.
    #[snafu(display("the source's length does not fit in 64 bits"))]
    TooLong,
}

/// The exact remaining length of `source`, as the length of its encoding.
async fn source_len<A>(
    source: &mut A,
) -> Result<u64, SourceLengthError<<A::Family as ExactLen>::LengthError>>
where
    A: ReceiverAccess<Family: ExactLen>,
    u64: TryFrom<<A::Family as ExactLen>::Length>,
{
    let length = source
        .remaining_len()
        .await
        .context(source_length_error::SourceSnafu)?;
    u64::try_from(length)
        .ok()
        .context(source_length_error::TooLongSnafu)
}

impl<A> EncodedLen for Chunks<A>
where
    A: ReceiverAccess<Family: ExactLen>,
    u64: TryFrom<<A::Family as ExactLen>::Length>,
{
    type LengthError = SourceLengthError<<A::Family as ExactLen>::LengthError>;
    async fn encoded_len(&mut self) -> Result<u64, Self::LengthError> {
        source_len(&mut self.source).await
    }
}
impl<A> EncodedLen for Fill<A>
where
    A: ReceiverAccess<Family: ExactLen>,
    u64: TryFrom<<A::Family as ExactLen>::Length>,
{
    type LengthError = SourceLengthError<<A::Family as ExactLen>::LengthError>;
    async fn encoded_len(&mut self) -> Result<u64, Self::LengthError> {
        source_len(&mut self.source).await
    }
}
impl<A, B> EncodedLen for Buffered<A, B>
where
    A: ReceiverAccess<Family: ExactLen>,
    u64: TryFrom<<A::Family as ExactLen>::Length>,
{
    type LengthError = SourceLengthError<<A::Family as ExactLen>::LengthError>;
    async fn encoded_len(&mut self) -> Result<u64, Self::LengthError> {
        source_len(&mut self.source).await
    }
}
