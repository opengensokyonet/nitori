//! Failures shared by the length-prefixed fields of TLS messages.
//!
//! TLS writes a variable-length field as a length of one, two or three bytes
//! followed by that many bytes (RFC 9846 §3.4). [`ReadVectorError`] is the
//! failure of reading such a field, [`ReadBoundedVectorError`] that of a
//! field whose length has a narrower range than its length field allows,
//! with [`LengthOutOfRangeError`] for the range. [`ExcessBytesError`] reports bytes
//! left over once a structure of known length has been read.

use crate::{
    alert::AlertDescription,
    message::HandshakeType,
    source::{read_bytes, read_u8, read_u16, read_u24},
    width::Width,
};
use bytes::{Buf, BufMut};
use core::{num::NonZeroUsize, ops::RangeInclusive};
use nitori_io::{SplitTo, error::IncompleteError};
use snafu::{ResultExt, Snafu};

/// A length lies outside the range its field allows.
///
/// It is constructed only when the check fails, so the length is always
/// outside [`min`](Self::min)`..=`[`max`](Self::max).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("length {len} is outside {min}..={max}"))]
pub struct LengthOutOfRangeError {
    len: usize,
    min: usize,
    max: usize,
}

impl LengthOutOfRangeError {
    /// The length.
    pub const fn length(self) -> usize {
        self.len
    }

    /// The smallest length allowed.
    pub const fn min(self) -> usize {
        self.min
    }

    /// The largest length allowed.
    pub const fn max(self) -> usize {
        self.max
    }

    /// Checks that `len` lies in `range`.
    pub(crate) fn check(len: usize, range: RangeInclusive<usize>) -> Result<(), Self> {
        if !range.contains(&len) {
            return Err(Self {
                len,
                min: *range.start(),
                max: *range.end(),
            });
        }
        Ok(())
    }
}

/// A structure of known length ends before its bytes do.
///
/// It is constructed only when bytes are left, so the excess is never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
#[snafu(display("{excess} bytes follow the last field"))]
pub struct ExcessBytesError {
    excess: NonZeroUsize,
}

impl ExcessBytesError {
    /// The number of bytes left over.
    pub const fn excess(self) -> NonZeroUsize {
        self.excess
    }

    /// Checks that no bytes are left of a structure, with `left` left.
    pub(crate) fn check(left: usize) -> Result<(), Self> {
        match NonZeroUsize::new(left) {
            Some(excess) => Err(Self { excess }),
            None => Ok(()),
        }
    }
}

/// Failures of reading a variable-length field whose length may be any
/// value of its length field.
#[derive(Debug, Snafu)]
#[snafu(module(read_vector_error), visibility(pub(crate)))]
pub enum ReadVectorError {
    /// The length field could not be read.
    #[snafu(display("vector length read failed"))]
    Length {
        /// The shortfall.
        source: IncompleteError,
    },
    /// The bytes the length field states are not all there.
    #[snafu(display("vector read failed"))]
    Bytes {
        /// The shortfall.
        source: IncompleteError,
    },
}

impl ReadVectorError {
    /// The alert the failure calls for: always decode_error.
    pub fn alert(&self) -> AlertDescription {
        AlertDescription::DECODE_ERROR
    }
}

/// Failures of reading a variable-length field whose length has a range
/// narrower than its length field.
#[derive(Debug, Snafu)]
#[snafu(module(read_bounded_vector_error), visibility(pub(crate)))]
pub enum ReadBoundedVectorError {
    /// The length field could not be read.
    #[snafu(display("vector length read failed"))]
    Length {
        /// The shortfall.
        source: IncompleteError,
    },
    /// The length lies outside the range of the field.
    #[snafu(transparent)]
    Range {
        /// The broken range.
        source: LengthOutOfRangeError,
    },
    /// The bytes the length field states are not all there.
    #[snafu(display("vector read failed"))]
    Bytes {
        /// The shortfall.
        source: IncompleteError,
    },
}

impl ReadBoundedVectorError {
    /// The alert the failure calls for: always decode_error.
    pub fn alert(&self) -> AlertDescription {
        AlertDescription::DECODE_ERROR
    }
}

/// Reads a length field of `width`.
pub(crate) fn read_length(src: &mut impl Buf, width: Width) -> Result<usize, IncompleteError> {
    Ok(match width {
        Width::U8 => usize::from(read_u8(src)?),
        Width::U16 => usize::from(read_u16(src)?),
        Width::U24 => read_u24(src)? as usize,
    })
}

/// Reads a length field of `width` and that many bytes, as a piece of the
/// source.
pub(crate) fn read_vector<S: Buf + SplitTo>(
    src: &mut S,
    width: Width,
) -> Result<S, ReadVectorError> {
    let len = read_length(src, width).context(read_vector_error::LengthSnafu)?;
    read_bytes(src, len).context(read_vector_error::BytesSnafu)
}

/// Reads a length field of `width`, checks that the length lies in `range`
/// and reads that many bytes, as a piece of the source.
pub(crate) fn read_bounded_vector<S: Buf + SplitTo>(
    src: &mut S,
    width: Width,
    range: RangeInclusive<usize>,
) -> Result<S, ReadBoundedVectorError> {
    let len = read_length(src, width).context(read_bounded_vector_error::LengthSnafu)?;
    LengthOutOfRangeError::check(len, range)?;
    read_bytes(src, len).context(read_bounded_vector_error::BytesSnafu)
}

/// Writes a length field of `width`.
///
/// The caller has checked that `len` fits the width.
pub(crate) fn write_length(dst: &mut impl BufMut, width: Width, len: usize) {
    debug_assert!(len <= width.max());
    let bytes = (len as u32).to_be_bytes();
    dst.put_slice(&bytes[4 - width.len()..]);
}

/// Writes a length field of `width` and `bytes`.
pub(crate) fn write_vector(dst: &mut impl BufMut, width: Width, bytes: &[u8]) {
    write_length(dst, width, bytes.len());
    dst.put_slice(bytes);
}

/// Writes the header of a handshake message whose body is `body_len` bytes
/// long.
pub(crate) fn write_header(dst: &mut impl BufMut, message: HandshakeType, body_len: usize) {
    dst.put_u8(message.get());
    write_length(dst, Width::U24, body_len);
}
