//! Reading from a source that holds the bytes of a whole message in memory.
//!
//! Every reader of the message contents reads from a [`Buf`]. A
//! variable-length field is read with [`SplitTo`] as a piece of the source
//! itself: a byte slice yields a subslice, and `Bytes` a piece that shares
//! its storage, so reading copies no field. A source that ends before a
//! field is complete is an [`IncompleteError`]; the bytes that were there
//! stay consumed.
use bytes::Buf;
use nitori_io::{SplitTo, error::IncompleteError};

/// The failure of a field of `expected` bytes, when `src` holds fewer;
/// consumes what is left.
pub(crate) fn incomplete(src: &mut impl Buf, expected: usize) -> IncompleteError {
    let completed = src.remaining();
    src.advance(completed);
    // invariant: every caller has found fewer than `expected` bytes left, so
    // `completed` is below `expected`.
    IncompleteError::new(expected, completed).expect("fewer bytes are left than the field needs")
}

/// Reads `N` bytes.
pub(crate) fn read_array<const N: usize>(src: &mut impl Buf) -> Result<[u8; N], IncompleteError> {
    if src.remaining() < N {
        return Err(incomplete(src, N));
    }
    let mut array = [0; N];
    src.copy_to_slice(&mut array);
    Ok(array)
}

/// Reads one byte.
pub(crate) fn read_u8(src: &mut impl Buf) -> Result<u8, IncompleteError> {
    let [byte] = read_array(src)?;
    Ok(byte)
}

/// Reads a two-byte integer.
pub(crate) fn read_u16(src: &mut impl Buf) -> Result<u16, IncompleteError> {
    Ok(u16::from_be_bytes(read_array(src)?))
}

/// Reads a three-byte integer.
pub(crate) fn read_u24(src: &mut impl Buf) -> Result<u32, IncompleteError> {
    let [a, b, c] = read_array(src)?;
    Ok(u32::from_be_bytes([0, a, b, c]))
}

/// Reads a four-byte integer.
pub(crate) fn read_u32(src: &mut impl Buf) -> Result<u32, IncompleteError> {
    Ok(u32::from_be_bytes(read_array(src)?))
}

/// Reads a field of `len` bytes as a piece of the source.
pub(crate) fn read_bytes<S: Buf + SplitTo>(src: &mut S, len: usize) -> Result<S, IncompleteError> {
    if src.remaining() < len {
        return Err(incomplete(src, len));
    }
    // invariant: `len` is at most what remains, the length of the source.
    Ok(src.split_to(len).expect("the source holds the field"))
}

/// Takes the rest of the source as a piece of its own.
pub(crate) fn take_rest<S: Buf + SplitTo>(src: &mut S) -> S {
    let len = src.remaining();
    // invariant: the remaining bytes are the length of the source.
    src.split_to(len)
        .expect("the source holds its remaining bytes")
}
