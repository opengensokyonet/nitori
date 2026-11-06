//! DATA frame parsing built from nitori_io operations.
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    calls::{ReadArrayError, ReadChunksExactError},
    prelude::*,
};
use snafu::{ResultExt, Snafu};
use std::{fmt, num::NonZeroUsize, ops::ControlFlow, pin::pin};

/// A frame type other than DATA.
///
/// The value is read from a variable-length integer, so it is at most
/// 2^62 - 1, and it is never 0, the type of DATA frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonDataFrameType(u64);

impl NonDataFrameType {
    /// The type as read from the frame header, or `None` for the type of DATA
    /// frames.
    fn new(frame_type: u64) -> Option<Self> {
        (frame_type != 0).then_some(Self(frame_type))
    }

    /// The type value: nonzero and at most 2^62 - 1.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for NonDataFrameType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Snafu)]
pub enum ReadVarintError<E> {
    #[snafu(display("could not read the first byte"))]
    FirstByte { source: ReadArrayError<E> },
    #[snafu(display("could not read the remaining bytes"))]
    RemainingBytes { source: ReadArrayError<E> },
}

#[derive(Debug, Snafu)]
pub enum CodecError<E> {
    #[snafu(display("could not read the frame type"))]
    FrameType { source: ReadVarintError<E> },
    #[snafu(display("expected DATA frame, got type {actual}"))]
    WrongType { actual: NonDataFrameType },
    #[snafu(display("could not read the frame length"))]
    FrameLength { source: ReadVarintError<E> },
    #[snafu(display("frame length does not fit in memory size"))]
    LengthTooLarge { source: std::num::TryFromIntError },
    #[snafu(display("could not read the frame payload"))]
    Payload { source: ReadChunksExactError<E> },
}

/// Append big-endian `bytes` to the low bits of a variable-length integer.
fn extend(value: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(value, |value, byte| (value << 8) | u64::from(*byte))
}

#[call]
pub async fn read_varint<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<u64, ReadVarintError<F::Error>> {
    let [first] = io.read_array::<1>().await.context(FirstByteSnafu)?;
    let value = u64::from(first & 0x3f);
    // The top two bits give the width: 1, 2, 4 or 8 bytes in all.
    Ok(match first >> 6 {
        0 => value,
        1 => extend(
            value,
            &io.read_array::<1>().await.context(RemainingBytesSnafu)?,
        ),
        2 => extend(
            value,
            &io.read_array::<3>().await.context(RemainingBytesSnafu)?,
        ),
        _ => extend(
            value,
            &io.read_array::<7>().await.context(RemainingBytesSnafu)?,
        ),
    })
}

#[call(sync, yields = F::Chunk)]
pub async fn read_data_frame<F: ReadChunk, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    chunk_size: NonZeroUsize,
) -> Result<usize, CodecError<F::Error>> {
    let frame_type = io.read_varint().await.context(FrameTypeSnafu)?;
    if let Some(actual) = NonDataFrameType::new(frame_type) {
        return WrongTypeSnafu { actual }.fail();
    }
    let length = io.read_varint().await.context(FrameLengthSnafu)?;
    let length = usize::try_from(length).context(LengthTooLargeSnafu)?;
    let mut chunks = pin!(io.read_chunks_exact(length, chunk_size));
    loop {
        match chunks.as_mut().step().await {
            ControlFlow::Continue(chunk) => yield chunk,
            ControlFlow::Break(result) => break result.context(PayloadSnafu)?,
        }
    }
    Ok(length)
}

#[cfg(test)]
mod tests;
