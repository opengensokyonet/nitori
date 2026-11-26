//! Write operations and their operation-specific failures.
use crate::{
    Combine, Write as WriteHost, WriteExt as _, WriteSegment,
    calls::EndianValue,
    error::{self, WriteZeroError},
};
use bytes::Buf;
use nitori_call::{ReceiverAccess, call};
use snafu::{IntoError, Snafu};

/// Accept all of `input`, every segment with the write options
/// `options`, and return the combined report of the segments accepted.
/// Pass `&mut buffer` to keep the cursor: after a failure or cancellation it
/// holds the unaccepted tail. Pass `()` to a sink that takes no options.
#[call(sync)]
pub async fn write_all<F, O, B>(
    io: &mut impl ReceiverAccess<Family = F>,
    input: B,
    options: O,
) -> Result<<F as WriteHost<O>>::Report, WriteAllError<F::Error>>
where
    F: WriteHost<O>,
    <F as WriteHost<O>>::Report: Combine,
    B: Buf,
{
    let mut input = input;
    let mut completed = 0;
    let mut combined = <F as WriteHost<O>>::Report::default();
    while input.has_remaining() {
        let before = input.remaining();
        match io.write(&mut input, &options).await {
            Ok(None) => return Err(error::WriteZeroSnafu { completed }.build().into()),
            Ok(Some(WriteSegment { count, report })) => {
                assert_eq!(
                    before.checked_sub(input.remaining()),
                    Some(count.get()),
                    "write cursor/count mismatch"
                );
                completed += count.get();
                combined = combined.combine(report);
            }
            Err(source) => {
                return Err(write_all_error::ReceiverSnafu { completed }.into_error(source));
            }
        }
    }
    Ok(combined)
}

/// Failures that [`write_all`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_all_error), visibility(pub(crate)))]
pub enum WriteAllError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError },
    #[snafu(display("write failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> WriteAllError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::WriteZero { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::WriteZero { .. } => None,
        }
    }
}

/// Write a fixed-size array completely with the write options
/// `options` and return the combined report.
#[call(sync)]
pub async fn write_array<const LENGTH: usize, F, O>(
    io: &mut impl ReceiverAccess<Family = F>,
    bytes: [u8; LENGTH],
    options: O,
) -> Result<<F as WriteHost<O>>::Report, WriteArrayError<F::Error>>
where
    F: WriteHost<O>,
    <F as WriteHost<O>>::Report: Combine,
{
    write_all(io, bytes.as_ref(), options)
        .await
        .map_err(WriteArrayError::from_write_all)
}

/// Failures that [`write_array`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_array_error), visibility(pub(crate)))]
pub enum WriteArrayError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError },
    #[snafu(display("write failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> WriteArrayError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::WriteZero { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::WriteZero { .. } => None,
        }
    }
}

impl<E> WriteArrayError<E> {
    /// Convert a failure of the underlying `write_all`.
    fn from_write_all(error: WriteAllError<E>) -> Self {
        match error {
            WriteAllError::WriteZero { source } => source.into(),
            WriteAllError::Receiver { source, completed } => {
                write_array_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Encode a primitive number in little endian, write it completely with the
/// write options `options` and return the combined report.
#[call(sync)]
pub async fn write_le<T: EndianValue, F, O>(
    io: &mut impl ReceiverAccess<Family = F>,
    value: T,
    options: O,
) -> Result<<F as WriteHost<O>>::Report, WriteLeError<F::Error>>
where
    F: WriteHost<O>,
    <F as WriteHost<O>>::Report: Combine,
{
    let bytes = value.to_le();
    write_all(io, bytes.as_ref(), options)
        .await
        .map_err(WriteLeError::from_write_all)
}

/// Failures that [`write_le`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_le_error), visibility(pub(crate)))]
pub enum WriteLeError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError },
    #[snafu(display("write failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> WriteLeError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::WriteZero { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::WriteZero { .. } => None,
        }
    }
}

impl<E> WriteLeError<E> {
    /// Convert a failure of the underlying `write_all`.
    fn from_write_all(error: WriteAllError<E>) -> Self {
        match error {
            WriteAllError::WriteZero { source } => source.into(),
            WriteAllError::Receiver { source, completed } => {
                write_le_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Encode a primitive number in big endian, write it completely with the
/// write options `options` and return the combined report.
#[call(sync)]
pub async fn write_be<T: EndianValue, F, O>(
    io: &mut impl ReceiverAccess<Family = F>,
    value: T,
    options: O,
) -> Result<<F as WriteHost<O>>::Report, WriteBeError<F::Error>>
where
    F: WriteHost<O>,
    <F as WriteHost<O>>::Report: Combine,
{
    let bytes = value.to_be();
    write_all(io, bytes.as_ref(), options)
        .await
        .map_err(WriteBeError::from_write_all)
}

/// Failures that [`write_be`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_be_error), visibility(pub(crate)))]
pub enum WriteBeError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZeroError },
    #[snafu(display("write failed after {completed} bytes"))]
    Receiver { source: E, completed: usize },
}

impl<E> WriteBeError<E> {
    /// Bytes completed by this operation before failure.
    pub fn completed(&self) -> usize {
        match self {
            Self::WriteZero { source } => source.completed,
            Self::Receiver { completed, .. } => *completed,
        }
    }

    /// The original host error, if the failure came from the host.
    pub fn host_error(&self) -> Option<&E> {
        match self {
            Self::Receiver { source, .. } => Some(source),
            Self::WriteZero { .. } => None,
        }
    }
}

impl<E> WriteBeError<E> {
    /// Convert a failure of the underlying `write_all`.
    fn from_write_all(error: WriteAllError<E>) -> Self {
        match error {
            WriteAllError::WriteZero { source } => source.into(),
            WriteAllError::Receiver { source, completed } => {
                write_be_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}
