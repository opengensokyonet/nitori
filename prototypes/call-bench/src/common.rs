//! The byte source, the frame format and the shared read primitive.
use bytes::BufMut;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};
use tokio::io::{AsyncRead, BufReader, DuplexStream, ReadBuf};

pub type Source = BufReader<DuplexStream>;

/// Read from an async reader into any `BufMut`, without initializing it.
pub fn poll_read_into<O: BufMut + ?Sized>(
    reader: Pin<&mut Source>,
    cx: &mut Context<'_>,
    destination: &mut O,
) -> Poll<io::Result<usize>> {
    let chunk = destination.chunk_mut();
    // SAFETY: ReadBuf only writes initialized bytes into the uninitialized slice.
    let mut buffer = ReadBuf::uninit(unsafe { chunk.as_uninit_slice_mut() });
    ready!(reader.poll_read(cx, &mut buffer))?;
    let count = buffer.filled().len();
    // SAFETY: `count` bytes were initialized by the read.
    unsafe { destination.advance_mut(count) };
    Poll::Ready(Ok(count))
}

/// A frame: a tag byte, a big-endian u16 length and that many payload bytes.
/// Lengths follow a fixed pseudo-random sequence over 0..=255.
pub fn encode(frames: usize) -> (Vec<u8>, Vec<usize>, u64) {
    let mut state = 0x2545_f491_u32;
    let mut bytes = Vec::new();
    let mut ends = Vec::new();
    let mut checksum = 0u64;
    for index in 0..frames {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let length = (state & 0xff) as u16;
        let tag = (index % 7) as u8;
        bytes.push(tag);
        bytes.extend(length.to_be_bytes());
        for offset in 0..length {
            bytes.push((offset as u8).wrapping_mul(31).wrapping_add(tag));
        }
        checksum += frame_checksum(tag, &bytes[bytes.len() - length as usize..]);
        ends.push(bytes.len());
    }
    (bytes, ends, checksum)
}

pub fn frame_checksum(tag: u8, payload: &[u8]) -> u64 {
    let sum: u64 = payload.iter().map(|&b| b as u64).sum();
    sum + tag as u64 * 1000 + payload.len() as u64 * 7
}
