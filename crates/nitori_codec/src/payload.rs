//! A source written out as the bytes of a value, one wrapper for each
//! pairing of the source's and the sink's capabilities, as the copies of
//! `nitori_io::calls` pair them.
//!
//! Wrap the source to choose how its bytes reach the sink: [`Chunks`] when
//! the source hands out owned chunks ([`ReadChunk`]) and the sink accepts
//! bytes ([`Write`]), [`Fill`] when the sink lends its storage
//! ([`WriteLent`]) to a source that reads into it ([`Read`]) and can
//! answer whether it has reached EOF ([`AtEnd`]), and
//! [`Buffered`], through a buffer of the caller's, when both only read and
//! accept bytes. A bare source is not a value: which of the three it would
//! take cannot be told from it, and a crate that keeps a source in a type of
//! its own encodes that type as it chooses.
//!
//! Each wrapper copies until the source ends, so bound the source to the
//! bytes it is to contribute first, such as with
//! [`ReadLimit`](nitori_io::adapters::ReadLimit); [`EncodedLen`] is the
//! source's exact remaining length ([`ExactLen`]), returned in the source's
//! length type and with its remainder error.
//!
//! As in the copies, the bytes are written with the metadata they were read
//! with as their write options: the sink takes the source's metadata
//! type, `()` for a source without metadata. Wrap a sink that takes other
//! options in [`MappedOptions`](nitori_io::adapters::MappedOptions) with a
//! [`MapOptions`](nitori_io::adapters::MapOptions) mapping or
//! [`FixedOptions`](nitori_io::adapters::FixedOptions).
use crate::{EncodeOn, EncodedLen};
use core::{future::Future, num::NonZeroUsize};
use nitori_call::ReceiverAccess;
use nitori_io::{
    AtEnd, Combine, ExactLen, ExactLenExt as _, Read, ReadChunk, Write, WriteLent,
    calls::{
        CopyBufferedError, CopyChunksError, CopyIntoLentError, CopyTotal, copy_buffered,
        copy_chunks, copy_into_lent,
    },
};

/// A source written out by writing its chunks, of at most `chunk_maximum`
/// bytes each: a source that hands out owned chunks ([`ReadChunk`]) into a
/// sink that accepts bytes ([`Write`]). Shared chunks, such as `Bytes`,
/// reach the sink without being copied.
///
/// Query the payload length through [`EncodedLen`].
///
/// ```compile_fail,E0277
/// use core::num::NonZeroUsize;
/// use nitori_call::ReceiverAccess;
/// use nitori_codec::{EncodedLen, KnownLen, payload::Chunks};
/// use nitori_io::ExactLen;
///
/// fn require_known(_: &impl KnownLen) {}
/// fn check<A: ReceiverAccess<Family: ExactLen>>(source: A) {
///     let mut payload: Chunks<A> = Chunks { source, chunk_maximum: NonZeroUsize::MIN };
///     drop(payload.encoded_len());
///     require_known(&payload);
/// }
/// ```
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
/// If the sink fills up, the source reports whether it has reached EOF
/// ([`AtEnd`]) without consuming bytes.
///
/// Query the payload length through [`EncodedLen`].
///
/// ```compile_fail,E0277
/// use core::num::NonZeroUsize;
/// use nitori_call::ReceiverAccess;
/// use nitori_codec::{EncodedLen, KnownLen, payload::Fill};
/// use nitori_io::ExactLen;
///
/// fn require_known(_: &impl KnownLen) {}
/// fn check<A: ReceiverAccess<Family: ExactLen>>(source: A) {
///     let mut payload: Fill<A> = Fill { source, chunk_maximum: NonZeroUsize::MIN };
///     drop(payload.encoded_len());
///     require_known(&payload);
/// }
/// ```
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
///
/// Query the payload length through [`EncodedLen`].
///
/// ```compile_fail,E0277
/// use nitori_call::ReceiverAccess;
/// use nitori_codec::{EncodedLen, KnownLen, payload::Buffered};
/// use nitori_io::ExactLen;
///
/// fn require_known(_: &impl KnownLen) {}
/// fn check<A: ReceiverAccess<Family: ExactLen>>(source: A) {
///     let mut payload: Buffered<A, [u8; 8]> = Buffered { source, buffer: [0u8; 8] };
///     drop(payload.encoded_len());
///     require_known(&payload);
/// }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Buffered<A, B> {
    /// The source.
    pub source: A,
    /// The buffer the bytes pass through.
    pub buffer: B,
}

impl<A, S, M> EncodeOn<S> for Chunks<A>
where
    A: ReceiverAccess<Family: ReadChunk<Metadata = M>>,
    M: Combine,
    S: ReceiverAccess<Family: Write<M, Report: Combine>>,
{
    type Output = CopyTotal<M, <S::Family as Write<M>>::Report>;
    type Error = CopyChunksError<<A::Family as Read>::Error, <S::Family as Write<M>>::Error>;
    async fn encode_on(mut self, sink: &mut S) -> Result<Self::Output, Self::Error> {
        copy_chunks(&mut self.source, sink, self.chunk_maximum).await
    }
}

impl<A, S, M> EncodeOn<S> for Fill<A>
where
    A: ReceiverAccess<Family: Read<Metadata = M> + AtEnd>,
    M: Combine + Clone,
    S: ReceiverAccess<Family: WriteLent<M>>,
    <S::Family as Write<M>>::Report: Combine,
{
    type Output = CopyTotal<M, <S::Family as Write<M>>::Report>;
    type Error = CopyIntoLentError<
        <A::Family as Read>::Error,
        <S::Family as Write<M>>::Error,
        <A::Family as AtEnd>::RemainderError,
    >;
    async fn encode_on(mut self, sink: &mut S) -> Result<Self::Output, Self::Error> {
        copy_into_lent(&mut self.source, sink, self.chunk_maximum).await
    }
}

impl<A, B, S, M> EncodeOn<S> for Buffered<A, B>
where
    A: ReceiverAccess<Family: Read<Metadata = M>>,
    M: Combine,
    B: AsMut<[u8]>,
    S: ReceiverAccess<Family: Write<M, Report: Combine>>,
{
    type Output = CopyTotal<M, <S::Family as Write<M>>::Report>;
    type Error = CopyBufferedError<<A::Family as Read>::Error, <S::Family as Write<M>>::Error>;
    async fn encode_on(mut self, sink: &mut S) -> Result<Self::Output, Self::Error> {
        copy_buffered(&mut self.source, sink, self.buffer.as_mut()).await
    }
}

/// The source's exact remaining length, in the source's own length type.
macro_rules! source_len {
    ($([$($g:tt)*] $ty:ty;)+) => {$(
        impl<$($g)*> EncodedLen for $ty
        where
            A: ReceiverAccess<Family: ExactLen>,
        {
            type Length = <A::Family as ExactLen>::Length;
            type LengthError = <A::Family as AtEnd>::RemainderError;
            fn encoded_len(&mut self) -> impl Future<Output = Result<Self::Length, Self::LengthError>> {
                self.source.remaining_len()
            }
        }
    )+};
}
source_len! {
    [A] Chunks<A>;
    [A] Fill<A>;
    [A, B] Buffered<A, B>;
}
