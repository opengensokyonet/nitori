//! Read operations and their operation-specific failures.
use crate::{
    Read as ReadHost, ReadChunk as ChunkHost, ReadChunkExt as _, ReadExt as _,
    error::{self, Incomplete, InsufficientCapacity},
};
use bytes::{Buf, BufMut};
use core::{num::NonZeroUsize, ops::ControlFlow, pin::pin};
use nitori_call::{ReceiverAccess, call};
use snafu::{IntoError, Snafu};

/// Fill exactly `length` bytes, checking the whole capacity before any IO.
#[call(sync)]
pub async fn read_exact<F, B>(
    io: &mut impl ReceiverAccess<Family = F>,
    destination: &mut B,
    length: usize,
) -> Result<(), ReadExactError<F::Error>>
where
    F: ReadHost,
    B: BufMut + ?Sized,
{
    let available = destination.remaining_mut();
    if available < length {
        return Err(error::InsufficientCapacitySnafu {
            required: length,
            available,
            completed: 0usize,
        }
        .build()
        .into());
    }
    fill(io, destination, length).await?;
    Ok(())
}

/// Failures that [`read_exact`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_exact_error), visibility(pub(crate)))]
pub enum ReadExactError<E> {
    #[snafu(transparent)]
    InsufficientCapacity { source: InsufficientCapacity },
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadExactError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::InsufficientCapacity { source } => source.completed,
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::InsufficientCapacity { .. } | Self::Incomplete { .. } => None,
        }
    }
}

impl<E> From<ReadArrayError<E>> for ReadExactError<E> {
    fn from(error: ReadArrayError<E>) -> Self {
        match error {
            ReadArrayError::Incomplete { source } => source.into(),
            ReadArrayError::Receiver { source, completed } => {
                read_exact_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Read until `length` bytes have been put into `destination`.
async fn fill<F, B>(
    io: &mut impl ReceiverAccess<Family = F>,
    mut destination: &mut B,
    length: usize,
) -> Result<(), ReadArrayError<F::Error>>
where
    F: ReadHost,
    B: BufMut + ?Sized,
{
    let mut completed = 0;
    while completed < length {
        let remaining = length - completed;
        match io.read(&mut (&mut destination).limit(remaining)).await {
            Ok(0) => {
                return Err(error::IncompleteSnafu {
                    expected: length,
                    completed,
                }
                .build()
                .into());
            }
            Ok(count) => {
                assert!(count <= remaining, "read exceeded limit");
                completed += count;
            }
            Err(source) => {
                return Err(read_array_error::ReceiverSnafu { completed }.into_error(source));
            }
        }
    }
    Ok(())
}

/// Append until EOF. A full destination before confirmed EOF is an error.
#[call(sync)]
pub async fn read_to_end<F, B>(
    io: &mut impl ReceiverAccess<Family = F>,
    destination: &mut B,
) -> Result<usize, ReadToEndError<F::Error>>
where
    F: ReadHost,
    B: BufMut + ?Sized,
{
    let mut completed = 0usize;
    loop {
        if destination.remaining_mut() == 0 {
            return Err(error::InsufficientCapacitySnafu {
                required: 1usize,
                available: 0usize,
                completed,
            }
            .build()
            .into());
        }
        match io.read(&mut *destination).await {
            Ok(0) => return Ok(completed),
            Ok(count) => {
                completed = completed
                    .checked_add(count)
                    .expect("read byte count overflow")
            }
            Err(source) => {
                return Err(read_to_end_error::ReceiverSnafu { completed }.into_error(source));
            }
        }
    }
}

/// Failures that [`read_to_end`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_to_end_error), visibility(pub(crate)))]
pub enum ReadToEndError<E> {
    #[snafu(transparent)]
    InsufficientCapacity { source: InsufficientCapacity },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadToEndError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::InsufficientCapacity { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::InsufficientCapacity { .. } => None,
        }
    }
}

/// Yield chunks until `maximum` bytes or EOF; the final value is the total.
#[call(sync, yields = F::Chunk)]
pub async fn read_chunks<F>(
    io: &mut impl ReceiverAccess<Family = F>,
    maximum: usize,
    chunk_maximum: NonZeroUsize,
) -> Result<usize, ReadChunksError<F::Error>>
where
    F: ChunkHost,
{
    let mut completed = 0;
    while let Some(request) = NonZeroUsize::new((maximum - completed).min(chunk_maximum.get())) {
        match io.read_chunk(request).await {
            Ok(Some(chunk)) => {
                let count = chunk.remaining();
                assert!(count > 0 && count <= request.get(), "invalid chunk length");
                completed += count;
                yield chunk;
            }
            Ok(None) => break,
            Err(source) => return Err(ReadChunksSnafu { completed }.into_error(source)),
        }
    }
    Ok(completed)
}

/// A host failure while yielding chunks; EOF is successful completion.
#[derive(Debug, Snafu)]
#[snafu(display("read failed after {completed} bytes"), visibility(pub(crate)))]
pub struct ReadChunksError<E> {
    pub source: E,
    pub completed: usize,
}

impl<E> ReadChunksError<E> {
    /// Bytes yielded before the host failure.
    pub fn completed(&self) -> usize {
        self.completed
    }

    /// The original host error. Every chunk failure has one.
    pub fn host_error(&self) -> &E {
        &self.source
    }
}

/// Yield exactly `length` bytes, reporting premature EOF with prior progress.
#[call(sync, yields = F::Chunk)]
pub async fn read_chunks_exact<F>(
    io: &mut impl ReceiverAccess<Family = F>,
    length: usize,
    chunk_maximum: NonZeroUsize,
) -> Result<(), ReadChunksExactError<F::Error>>
where
    F: ChunkHost,
{
    let mut chunks = pin!(io.read_chunks(length, chunk_maximum));
    loop {
        match chunks.as_mut().next().await {
            Some(ControlFlow::Continue(chunk)) => yield chunk,
            Some(ControlFlow::Break(Ok(completed))) if completed == length => return Ok(()),
            Some(ControlFlow::Break(Ok(completed))) => {
                return Err(error::IncompleteSnafu {
                    expected: length,
                    completed,
                }
                .build()
                .into());
            }
            Some(ControlFlow::Break(Err(error))) => {
                return Err(read_chunks_exact_error::ReceiverSnafu {
                    completed: error.completed,
                }
                .into_error(error.source));
            }
            None => unreachable!("the final value ends the stream"),
        }
    }
}

/// Failures that [`read_chunks_exact`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_chunks_exact_error), visibility(pub(crate)))]
pub enum ReadChunksExactError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadChunksExactError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

/// Read a fixed-size array without selecting a chunk representation.
#[call(sync)]
pub async fn read_array<const LENGTH: usize, F>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<[u8; LENGTH], ReadArrayError<F::Error>>
where
    F: ReadHost,
{
    let mut bytes = [0; LENGTH];
    fill(io, &mut bytes.as_mut_slice(), LENGTH).await?;
    Ok(bytes)
}

/// Failures that [`read_array`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_array_error), visibility(pub(crate)))]
pub enum ReadArrayError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadArrayError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

/// Decode a primitive number in little endian after reading its complete array.
#[call(sync)]
pub async fn read_le<T: EndianValue, F>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<T, ReadLeError<F::Error>>
where
    F: ReadHost,
{
    let mut bytes = T::Bytes::default();
    let length = bytes.as_mut().len();
    fill(io, &mut bytes.as_mut(), length).await?;
    Ok(T::from_le(bytes))
}

/// Failures that [`read_le`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_le_error), visibility(pub(crate)))]
pub enum ReadLeError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadLeError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

impl<E> From<ReadArrayError<E>> for ReadLeError<E> {
    fn from(error: ReadArrayError<E>) -> Self {
        match error {
            ReadArrayError::Incomplete { source } => source.into(),
            ReadArrayError::Receiver { source, completed } => {
                read_le_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Decode a primitive number in big endian after reading its complete array.
#[call(sync)]
pub async fn read_be<T: EndianValue, F>(
    io: &mut impl ReceiverAccess<Family = F>,
) -> Result<T, ReadBeError<F::Error>>
where
    F: ReadHost,
{
    let mut bytes = T::Bytes::default();
    let length = bytes.as_mut().len();
    fill(io, &mut bytes.as_mut(), length).await?;
    Ok(T::from_be(bytes))
}

/// Failures that [`read_be`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(read_be_error), visibility(pub(crate)))]
pub enum ReadBeError<E> {
    #[snafu(transparent)]
    Incomplete { source: Incomplete },
    #[snafu(display("read failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> ReadBeError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::Incomplete { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::Incomplete { .. } => None,
        }
    }
}

impl<E> From<ReadArrayError<E>> for ReadBeError<E> {
    fn from(error: ReadArrayError<E>) -> Self {
        match error {
            ReadArrayError::Incomplete { source } => source.into(),
            ReadArrayError::Receiver { source, completed } => {
                read_be_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Sealed implementation detail selecting a concrete array for each number,
/// shared by the endian reads and writes.
/// Implemented for integers (including pointer-sized integers), f32, and f64.
pub trait EndianValue: private::Sealed + Sized {
    #[doc(hidden)]
    type Bytes: AsRef<[u8]> + AsMut<[u8]> + Default;
    #[doc(hidden)]
    fn from_le(bytes: Self::Bytes) -> Self;
    #[doc(hidden)]
    fn from_be(bytes: Self::Bytes) -> Self;
    #[doc(hidden)]
    fn to_le(self) -> Self::Bytes;
    #[doc(hidden)]
    fn to_be(self) -> Self::Bytes;
}
mod private {
    pub trait Sealed {}
}
macro_rules! endian_impl {
    ($($number:ty),* $(,)?) => {$ (
        impl private::Sealed for $number {}
        impl EndianValue for $number {
            type Bytes = [u8; core::mem::size_of::<Self>()];
            fn from_le(bytes: Self::Bytes) -> Self {
                <$number>::from_le_bytes(bytes)
            }
            fn from_be(bytes: Self::Bytes) -> Self {
                <$number>::from_be_bytes(bytes)
            }
            fn to_le(self) -> Self::Bytes {
                self.to_le_bytes()
            }
            fn to_be(self) -> Self::Bytes {
                self.to_be_bytes()
            }
        }
    )*};
}
endian_impl!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);
