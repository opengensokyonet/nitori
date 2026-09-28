//! DATA frame parsing built from nitori_io operations.
use nitori_call::{Target, call};
use nitori_io::{
    Read, ReadChunk, TargetChunkExt, TargetReadExt,
    calls::{ReadBeError, ReadChunksExactError},
};
use snafu::{ResultExt, Snafu};
use std::{num::NonZeroUsize, ops::CoroutineState};

#[derive(Debug, Snafu)]
pub enum FrameError<E> {
    #[snafu(display("could not read frame header"))]
    Header { source: ReadBeError<E> },
    #[snafu(display("expected DATA frame, got type {actual}"))]
    WrongType { actual: u64 },
    #[snafu(display("frame length does not fit in memory size"))]
    Length { source: std::num::TryFromIntError },
    #[snafu(display("could not read frame payload"))]
    Payload { source: ReadChunksExactError<E> },
}

#[call]
async fn read_varint<H: Read>(io: Target<H>) -> Result<u64, ReadBeError<H::Error>> {
    let first = io.read_be::<u8>().await?;
    let length = 1usize << (first >> 6);
    let mut value = u64::from(first & 0x3f);
    for _ in 1..length {
        value = (value << 8) | u64::from(io.read_be::<u8>().await?);
    }
    Ok(value)
}

#[call(sync, yields = H::Chunk)]
pub async fn read_data_frame<H: ReadChunk>(
    io: Target<H>,
    chunk_size: NonZeroUsize,
) -> Result<usize, FrameError<H::Error>> {
    let frame_type = io.read_varint().await.context(HeaderSnafu)?;
    if frame_type != 0 {
        return WrongTypeSnafu { actual: frame_type }.fail();
    }
    let length = io.read_varint().await.context(HeaderSnafu)?;
    let length = usize::try_from(length).context(LengthSnafu)?;
    let mut chunks = std::pin::pin!(io.read_chunks_exact(length, chunk_size));
    while let Some(event) = chunks.as_mut().next().await {
        match event {
            CoroutineState::Yielded(chunk) => yield chunk,
            CoroutineState::Complete(result) => result.context(PayloadSnafu)?,
        }
    }
    Ok(length)
}

#[cfg(test)]
mod tests;
