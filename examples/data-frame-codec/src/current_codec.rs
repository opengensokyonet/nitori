//! DATA frame parsing built from nitori_io operations.
use nitori_call::{ReceiverAccess, call};
use nitori_io::{
    Read, ReadChunk,
    calls::{ReadBeError, ReadBeExt as _, ReadChunksExactError, ReadChunksExactExt as _},
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
pub async fn read_varint<A: ReceiverAccess<Family: Read>>(
    io: &mut A,
) -> Result<u64, ReadBeError<<A::Family as Read>::Error>> {
    let first = io.read_be::<u8>().await?;
    let length = 1usize << (first >> 6);
    let mut value = u64::from(first & 0x3f);
    for _ in 1..length {
        value = (value << 8) | u64::from(io.read_be::<u8>().await?);
    }
    Ok(value)
}

#[call(sync, yields = <A::Family as ReadChunk>::Chunk)]
pub async fn read_data_frame<A: ReceiverAccess<Family: ReadChunk>>(
    io: &mut A,
    chunk_size: NonZeroUsize,
) -> Result<usize, CodecError<<A::Family as Read>::Error>> {
    let frame_type = io.read_varint().await.context(HeaderSnafu)?;
    if frame_type != 0 {
        return WrongTypeSnafu { actual: frame_type }.fail();
    }
    let length = io.read_varint().await.context(HeaderSnafu)?;
    let length = usize::try_from(length).context(LengthSnafu)?;
    let mut chunks = pin!(io.read_chunks_exact(length, chunk_size));
    while let Some(event) = chunks.as_mut().next().await {
        match event {
            ControlFlow::Continue(chunk) => yield chunk,
            ControlFlow::Break(result) => result.context(PayloadSnafu)?,
        }
    }
    Ok(length)
}

#[cfg(test)]
mod tests;
