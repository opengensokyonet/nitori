//! Write operations and their operation-specific failures.
use crate::{
    Write as WriteHost, WriteExt as _,
    calls::EndianValue,
    error::{self, WriteZero},
};
use bytes::Buf;
use nitori_call::{ReceiverAccess, call};
use snafu::{IntoError, Snafu};

/// A write completion returns the remaining input even on error.
#[derive(Debug)]
pub struct WriteReturn<B, E> {
    pub input: B,
    pub result: Result<usize, E>,
}

/// Accept all input, preserving the unaccepted tail and count on failure.
#[call(sync)]
pub async fn write_all<A, B>(
    io: &mut A,
    input: B,
) -> WriteReturn<B, WriteAllError<<A::Family as WriteHost>::Error>>
where
    A: ReceiverAccess<Family: WriteHost>,
    B: Buf,
{
    let mut input = input;
    let mut completed = 0;
    let result = loop {
        if !input.has_remaining() {
            break Ok(completed);
        }
        let before = input.remaining();
        match io.write(&mut input).await {
            Ok(0) => break Err(error::WriteZeroSnafu { completed }.build().into()),
            Ok(count) => {
                assert_eq!(
                    before.checked_sub(input.remaining()),
                    Some(count),
                    "write cursor/count mismatch"
                );
                completed += count;
            }
            Err(source) => {
                break Err(write_all_error::ReceiverSnafu { completed }.into_error(source));
            }
        }
    };
    WriteReturn { input, result }
}

/// Failures that [`write_all`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_all_error), visibility(pub(crate)))]
pub enum WriteAllError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZero },
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

/// Write a fixed-size array completely.
#[call(sync)]
pub async fn write_array<const LENGTH: usize, A>(
    io: &mut A,
    bytes: [u8; LENGTH],
) -> Result<(), WriteArrayError<<A::Family as WriteHost>::Error>>
where
    A: ReceiverAccess<Family: WriteHost>,
{
    write_all(io, bytes.as_ref()).await.result?;
    Ok(())
}

/// Failures that [`write_array`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_array_error), visibility(pub(crate)))]
pub enum WriteArrayError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZero },
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

impl<E> From<WriteAllError<E>> for WriteArrayError<E> {
    fn from(error: WriteAllError<E>) -> Self {
        match error {
            WriteAllError::WriteZero { source } => source.into(),
            WriteAllError::Receiver { source, completed } => {
                write_array_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Encode a primitive number in little endian and write it completely.
#[call(sync)]
pub async fn write_le<T: EndianValue, A>(
    io: &mut A,
    value: T,
) -> Result<(), WriteLeError<<A::Family as WriteHost>::Error>>
where
    A: ReceiverAccess<Family: WriteHost>,
{
    let bytes = value.to_le();
    write_all(io, bytes.as_ref()).await.result?;
    Ok(())
}

/// Failures that [`write_le`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_le_error), visibility(pub(crate)))]
pub enum WriteLeError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZero },
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

impl<E> From<WriteAllError<E>> for WriteLeError<E> {
    fn from(error: WriteAllError<E>) -> Self {
        match error {
            WriteAllError::WriteZero { source } => source.into(),
            WriteAllError::Receiver { source, completed } => {
                write_le_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}

/// Encode a primitive number in big endian and write it completely.
#[call(sync)]
pub async fn write_be<T: EndianValue, A>(
    io: &mut A,
    value: T,
) -> Result<(), WriteBeError<<A::Family as WriteHost>::Error>>
where
    A: ReceiverAccess<Family: WriteHost>,
{
    let bytes = value.to_be();
    write_all(io, bytes.as_ref()).await.result?;
    Ok(())
}

/// Failures that [`write_be`] can produce.
#[derive(Debug, Snafu)]
#[snafu(module(write_be_error), visibility(pub(crate)))]
pub enum WriteBeError<E> {
    #[snafu(transparent)]
    WriteZero { source: WriteZero },
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

impl<E> From<WriteAllError<E>> for WriteBeError<E> {
    fn from(error: WriteAllError<E>) -> Self {
        match error {
            WriteAllError::WriteZero { source } => source.into(),
            WriteAllError::Receiver { source, completed } => {
                write_be_error::ReceiverSnafu { completed }.into_error(source)
            }
        }
    }
}
