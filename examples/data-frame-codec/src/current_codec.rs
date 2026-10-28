//! DATA frame parsing built from nitori_io operations.
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    calls::{ReadBeError, ReadChunksExactError},
    prelude::*,
};
use snafu::{ResultExt, Snafu};
use std::{num::NonZeroUsize, ops::ControlFlow, pin::pin};

#[derive(Debug, Snafu)]
pub enum CodecError<E> {
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
pub async fn read_varint<F: Read, A: ReceiverAccess<Family = F>>(
    io: &mut A,
) -> Result<u64, ReadBeError<F::Error>> {
    let first = io.read_be::<u8>().await?;
    let length = 1usize << (first >> 6);
    let mut value = u64::from(first & 0x3f);
    for _ in 1..length {
        value = (value << 8) | u64::from(io.read_be::<u8>().await?);
    }
    Ok(value)
}

#[call(sync, yields = F::Chunk)]
pub async fn read_data_frame<F: ReadChunk, A: ReceiverAccess<Family = F>>(
    io: &mut A,
    chunk_size: NonZeroUsize,
) -> Result<usize, CodecError<F::Error>> {
    let frame_type = io.read_varint().await.context(HeaderSnafu)?;
    if frame_type != 0 {
        return WrongTypeSnafu { actual: frame_type }.fail();
    }
    let length = io.read_varint().await.context(HeaderSnafu)?;
    let length = usize::try_from(length).context(LengthSnafu)?;
    let mut chunks = pin!(io.read_chunks_exact(length, chunk_size));
    loop {
        match chunks.as_mut().next_event().await {
            ControlFlow::Continue(chunk) => yield chunk,
            ControlFlow::Break(result) => break result.context(PayloadSnafu)?,
        }
    }
    Ok(length)
}

#[cfg(test)]
mod tests;
